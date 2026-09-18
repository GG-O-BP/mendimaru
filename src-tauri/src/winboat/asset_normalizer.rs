//! Opt-in repair of generated *inputs* to Studio's Rspack watcher. Dist output
//! is never patched: Rspack must resolve the widget and extract its real CSS.
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, MetadataExt, OpenOptions, OpenOptionsExt};
use regex::Regex;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ENTRIES: usize = 10_000;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 8;

#[derive(Clone, PartialEq, Eq)]
struct Revision {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

fn revision(metadata: &cap_std::fs::Metadata) -> Revision {
    Revision {
        device: metadata.dev(),
        inode: metadata.ino(),
        length: metadata.len(),
        modified: (metadata.mtime(), metadata.mtime_nsec()),
        changed: (metadata.ctime(), metadata.ctime_nsec()),
    }
}

#[derive(Default, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Scan {
    pub rewritten_files: usize,
    pub rewritten_imports: usize,
    pub pending_files: usize,
}

pub(crate) struct Normalizer {
    project: Dir,
    imports: Regex,
    requires: Regex,
    action_unc: String,
    observed: HashMap<PathBuf, Revision>,
    completed: HashMap<PathBuf, Revision>,
}

#[derive(Debug)]
struct Diagnostic(&'static str);

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}
impl std::error::Error for Diagnostic {}

pub(crate) fn diagnostic(error: &io::Error) -> Option<&'static str> {
    error
        .get_ref()?
        .downcast_ref::<Diagnostic>()
        .map(|value| value.0)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, Diagnostic(message))
}

/// Traverse through anchored directory descriptors, refusing symlinks even in
/// intermediate components. Keep the selected project identity for this run.
fn open_absolute(path: &Path) -> io::Result<Dir> {
    if !path.is_absolute() {
        return Err(invalid("the shared directory must be absolute"));
    }
    let mut directory = Dir::open_ambient_dir("/", cap_std::ambient_authority())?;
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => directory = directory.open_dir_nofollow(name)?,
            _ => return Err(invalid("the shared directory must be direct")),
        }
    }
    Ok(directory)
}

impl Normalizer {
    pub(crate) fn new(shared: &Path, project: &Path, windows_share: &str) -> io::Result<Self> {
        if !windows_share.eq_ignore_ascii_case(r"\\host.lan\Data") {
            return Err(invalid(
                "asset normalization requires the WinBoat host.lan Data share",
            ));
        }
        let relative = project.strip_prefix(shared).map_err(|_| {
            invalid("asset normalization requires a project in the configured shared workspace")
        })?;
        let mut directory = open_absolute(shared)?;
        let mut names = Vec::new();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(invalid("the selected project path must be direct"));
            };
            let name = name
                .to_str()
                .ok_or_else(|| invalid("the project path must be UTF-8"))?;
            if name.contains(['"', '\'', '\\', '\n', '\r', '%', '?', '#']) {
                return Err(invalid(
                    "the project path cannot be represented safely in generated imports",
                ));
            }
            directory = directory.open_dir_nofollow(name)?;
            names.push(name);
        }
        if names.is_empty() {
            return Err(invalid("the project must be below the shared workspace"));
        }
        let prefix = format!(
            "//host.lan/Data/{}/deployment/web/widgets/",
            names.join("/")
        );
        let action_prefix = format!("//host.lan/Data/{}/javascriptsource/", names.join("/"));
        let action_unc =
            format!(r"\\host.lan\Data\{}\javascriptsource\", names.join(r"\")).replace('\\', r"\\");
        // Generated, single-line references only. Do not change arbitrary
        // strings, comments, widget sources, another project, or user sources.
        let imports = Regex::new(&format!(
            r#"(?m)^(?P<before>[ \t]*import[ \t]+(?:[A-Za-z0-9_*$,{{}} \t]+[ \t]+from[ \t]+)?)(?P<quote>["'])(?P<url>{})(?P<asset>[^"'\r\n]+)(?P<end>["'];[ \t]*\r?$)"#,
            regex::escape(&prefix),
        )).map_err(|_| invalid("the generated import matcher could not be prepared"))?;
        let requires = Regex::new(&format!(
            r#"(?m)^(?P<before>[ \t]*(?:"action"[ \t]*:[ \t]*)?\(\)[ \t]*=>[ \t]*require\()(?P<quote>["'])(?P<url>{})(?P<asset>[^"'\r\n]+)(?P<end>["']\)\.[A-Za-z_$][A-Za-z0-9_$]*,?[ \t]*\r?$)"#,
            regex::escape(&action_prefix),
        )).map_err(|_| invalid("the generated JavaScript action matcher could not be prepared"))?;
        Ok(Self {
            project: directory,
            imports,
            requires,
            action_unc,
            observed: HashMap::new(),
            completed: HashMap::new(),
        })
    }

    pub(crate) fn scan(&mut self) -> io::Result<Scan> {
        match self.scan_inner() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Studio can remove/replace the deployment tree during any
                // directory or file operation. Re-discover it on the next tick.
                self.observed.clear();
                self.completed.clear();
                Ok(Scan {
                    pending_files: 1,
                    ..Scan::default()
                })
            }
            result => result,
        }
    }

    fn scan_inner(&mut self) -> io::Result<Scan> {
        let mut result = Scan::default();
        let mut observed = HashMap::new();
        let mut budget = (0, 0);
        let web = match self
            .project
            .open_dir_nofollow("deployment")
            .and_then(|dir| dir.open_dir_nofollow("web"))
        {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.observed.clear();
                self.completed.clear();
                return Ok(result);
            }
            Err(error) => return Err(error),
        };
        // These generated directories contain static widget imports and
        // generated nanoflow action references. Keep original sources intact.
        for name in ["layouts", "pages", "nanoflows"] {
            match web.open_dir_nofollow(name) {
                Ok(directory) => self.scan_directory(
                    &directory,
                    Path::new(name),
                    1,
                    &mut budget,
                    &mut observed,
                    &mut result,
                )?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        self.completed.retain(|path, _| observed.contains_key(path));
        self.observed = observed;
        Ok(result)
    }

    fn scan_directory(
        &mut self,
        directory: &Dir,
        relative: &Path,
        depth: usize,
        budget: &mut (usize, u64),
        observed: &mut HashMap<PathBuf, Revision>,
        result: &mut Scan,
    ) -> io::Result<()> {
        if depth > MAX_DEPTH {
            return Err(invalid(
                "generated asset directory nesting exceeds the limit",
            ));
        }
        for entry in directory.entries()? {
            let entry = entry?;
            budget.0 += 1;
            if budget.0 > MAX_ENTRIES {
                return Err(invalid("generated asset entry count exceeds the limit"));
            }
            let name = entry.file_name();
            let metadata = match directory.symlink_metadata(&name) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if metadata.file_type().is_symlink() {
                return Err(invalid("generated asset symlinks are unsupported"));
            }
            let path = relative.join(&name);
            if metadata.is_dir() {
                self.scan_directory(
                    &directory.open_dir_nofollow(&name)?,
                    &path,
                    depth + 1,
                    budget,
                    observed,
                    result,
                )?;
            } else if path.extension() == Some(OsStr::new("js")) {
                if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_FILE_BYTES {
                    return Err(invalid(
                        "generated assets must be direct, single-link files of at most 8 MiB",
                    ));
                }
                let current = revision(&metadata);
                observed.insert(path.clone(), current.clone());
                if self.completed.get(&path) == Some(&current) {
                    continue;
                }
                if self.observed.get(&path) != Some(&current) {
                    result.pending_files += 1;
                    continue;
                }
                // A file must be unchanged across two scans before reading.
                budget.1 += metadata.len();
                if budget.1 > MAX_SCAN_BYTES {
                    return Err(invalid(
                        "generated asset scan exceeds the 64 MiB read limit",
                    ));
                }
                let bytes = read_direct(directory, &name)?;
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| invalid("generated JavaScript must be UTF-8"))?;
                let (normalized, count) = self.normalize(text, depth)?;
                if count > 0 {
                    // Compare both bytes and identity just before an atomic
                    // replacement; do not truncate a file Rspack might read.
                    if read_direct(directory, &name)? != bytes
                        || revision(&directory.symlink_metadata(&name)?) != current
                    {
                        result.pending_files += 1;
                        continue;
                    }
                    atomic_replace(
                        directory,
                        &name,
                        normalized.as_bytes(),
                        metadata.permissions(),
                    )?;
                    result.rewritten_files += 1;
                    result.rewritten_imports += count;
                    let updated = revision(&directory.symlink_metadata(&name)?);
                    observed.insert(path.clone(), updated.clone());
                    self.completed.insert(path, updated);
                } else {
                    self.completed.insert(path, current);
                }
            }
        }
        Ok(())
    }

    fn normalize(&self, text: &str, depth: usize) -> io::Result<(String, usize)> {
        let (text, import_count) = self.normalize_widget_imports(text, depth)?;
        let (text, require_count) = self.normalize_nanoflow_requires(&text)?;
        Ok((text, import_count + require_count))
    }

    fn normalize_widget_imports(&self, text: &str, depth: usize) -> io::Result<(String, usize)> {
        let mut result = String::with_capacity(text.len());
        let mut offset = 0;
        let mut count = 0;
        let mut lexer = ImportContext::default();
        for capture in self.imports.captures_iter(text) {
            let matched = capture.get(0).expect("complete import match");
            if !lexer.is_code_at(text.as_bytes(), matched.start()) {
                continue;
            }
            let asset = &capture["asset"];
            if capture["quote"] != capture["end"][..1]
                || asset.contains(['\\', '%', '?', '#', '\0'])
                || asset
                    .split('/')
                    .any(|part| part.is_empty() || matches!(part, "." | ".."))
                || !matches!(
                    Path::new(asset).extension().and_then(OsStr::to_str),
                    Some("js" | "mjs" | "css")
                )
            {
                return Err(invalid(
                    "the generated widget import has an unsupported path",
                ));
            }
            result.push_str(&text[offset..matched.start()]);
            result.push_str(&capture["before"]);
            result.push_str(&capture["quote"]);
            result.push_str(&"../".repeat(depth));
            result.push_str("widgets/");
            result.push_str(asset);
            result.push_str(&capture["end"]);
            offset = matched.end();
            count += 1;
        }
        result.push_str(&text[offset..]);
        Ok((result, count))
    }

    fn normalize_nanoflow_requires(&self, text: &str) -> io::Result<(String, usize)> {
        let mut result = String::with_capacity(text.len());
        let mut offset = 0;
        let mut count = 0;
        let mut lexer = ImportContext::default();
        for capture in self.requires.captures_iter(text) {
            let matched = capture.get(0).expect("complete action require match");
            if !lexer.is_code_at(text.as_bytes(), matched.start()) {
                continue;
            }
            let asset = &capture["asset"];
            if capture["quote"] != capture["end"][..1]
                || asset.contains(['\\', '%', '?', '#', '\0'])
                || asset.is_empty()
                || asset.ends_with(".js")
                || asset
                    .split('/')
                    .any(|part| part.is_empty() || matches!(part, "." | ".."))
            {
                return Err(invalid(
                    "the generated JavaScript action path is unsupported",
                ));
            }
            result.push_str(&text[offset..matched.start()]);
            result.push_str(&capture["before"]);
            result.push_str(&capture["quote"]);
            result.push_str(&self.action_unc);
            result.push_str(&asset.replace('/', r"\\"));
            result.push_str(".js");
            result.push_str(&capture["end"]);
            offset = matched.end();
            count += 1;
        }
        result.push_str(&text[offset..]);
        Ok((result, count))
    }
}

/// Generated imports precede template/JSX bodies. Ignore quoted/commented
/// lookalikes; after a template begins, conservatively leave the rest alone.
#[derive(Default)]
struct ImportContext {
    cursor: usize,
    state: u8,
}

impl ImportContext {
    fn is_code_at(&mut self, bytes: &[u8], end: usize) -> bool {
        while self.cursor < end {
            let current = bytes[self.cursor];
            let next = bytes.get(self.cursor + 1).copied();
            match self.state {
                b'`' => return false,
                b'\'' | b'"' => {
                    if current == b'\\' {
                        self.cursor += 1;
                    } else if current == self.state {
                        self.state = 0;
                    }
                }
                b'/' => {
                    if current == b'\n' {
                        self.state = 0;
                    }
                }
                b'*' => {
                    if current == b'*' && next == Some(b'/') {
                        self.state = 0;
                        self.cursor += 1;
                    }
                }
                _ => match current {
                    b'\'' | b'"' | b'`' => self.state = current,
                    b'/' if matches!(next, Some(b'/' | b'*')) => {
                        self.state = next.unwrap();
                        self.cursor += 1;
                    }
                    _ => {}
                },
            }
            self.cursor += 1;
        }
        self.state == 0
    }
}

fn read_direct(directory: &Dir, name: &OsStr) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK);
    let file = directory.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.len() > MAX_FILE_BYTES {
        return Err(invalid(
            "the generated file changed to an unsupported type or size",
        ));
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(invalid("the generated file exceeds 8 MiB"));
    }
    Ok(bytes)
}

fn atomic_replace(
    directory: &Dir,
    name: &OsStr,
    bytes: &[u8],
    permissions: cap_std::fs::Permissions,
) -> io::Result<()> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|_| io::Error::other("asset temporary name generation failed"))?;
    let temporary = format!(".mendimaru-{:032x}.tmp", u128::from_le_bytes(nonce));
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No)
        .mode(0o600);
    let result = (|| {
        let mut file = directory.open_with(&temporary, &options)?;
        file.write_all(bytes)?;
        file.set_permissions(permissions)?;
        file.sync_all()?;
        directory.rename(&temporary, directory, name)
    })();
    if result.is_err() {
        let _ = directory.remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn fixture() -> (tempfile::TempDir, Normalizer) {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("Project")).unwrap();
        let normalizer = Normalizer::new(
            root.path(),
            &root.path().join("Project"),
            r"\\host.lan\Data",
        )
        .unwrap();
        (root, normalizer)
    }

    const ORIGINAL: &str = "import * as Widget from \"//host.lan/Data/Project/deployment/web/widgets/com/widget.mjs\";\r\nimport \"//host.lan/Data/Project/deployment/web/widgets/com/widget.css\";\r\n";
    const NORMALIZED: &str = "import * as Widget from \"../widgets/com/widget.mjs\";\r\nimport \"../widgets/com/widget.css\";\r\n";
    const ACTION_ORIGINAL: &str = "      \"action\": () => require(\"//host.lan/Data/Project/javascriptsource/atlas_core/actions/ReloadWithState\").ReloadWithState,\r\n";
    const ACTION_NORMALIZED: &str = concat!(
        r#"      "action": () => require("\\\\host.lan\\Data\\Project\\javascriptsource\\atlas_core\\actions\\ReloadWithState.js").ReloadWithState,"#,
        "\r\n"
    );

    #[test]
    fn normalizes_only_selected_widget_static_imports_at_the_correct_depth() {
        let (_root, normalizer) = fixture();
        assert_eq!(
            normalizer.normalize(ORIGINAL, 1).unwrap(),
            (NORMALIZED.to_string(), 2)
        );
        assert_eq!(
            normalizer.normalize(ORIGINAL, 2).unwrap().0,
            NORMALIZED.replace("../widgets", "../../widgets")
        );
        let block = format!("/*\n{ORIGINAL}*/\nconst text = `\n{ORIGINAL}`;\n");
        assert_eq!(normalizer.normalize(&block, 1).unwrap(), (block, 0));
        let untouched = [
            "// import \"//host.lan/Data/Project/deployment/web/widgets/a.css\";",
            "const value = '//host.lan/Data/Project/deployment/web/widgets/a.css';",
            "import '//host.lan/Data/Other/deployment/web/widgets/a.css';",
            "import 'https://example.test/widget.mjs';",
            "import '../widgets/a.css';",
            "import '//host.lan/Data/Project/javascriptsource/module/a.js';",
        ]
        .join("\n");
        assert_eq!(normalizer.normalize(&untouched, 1).unwrap(), (untouched, 0));
        for suffix in [
            "../secret.js",
            "a/../../secret.js",
            "%2e%2e/secret.js",
            "a.css?x=1",
            "a\\b.mjs",
            "a.svg",
        ] {
            let source =
                format!("import '//host.lan/Data/Project/deployment/web/widgets/{suffix}';");
            assert!(normalizer.normalize(&source, 1).is_err());
        }
    }

    #[test]
    fn normalizes_only_generated_nanoflow_javascript_action_requires() {
        let (_root, normalizer) = fixture();
        assert_eq!(
            normalizer.normalize(ACTION_ORIGINAL, 1).unwrap(),
            (ACTION_NORMALIZED.to_string(), 1)
        );
        let block = format!("/*\n{ACTION_ORIGINAL}*/\nconst text = `\n{ACTION_ORIGINAL}`;\n");
        assert_eq!(normalizer.normalize(&block, 1).unwrap(), (block, 0));
        let untouched = [
            "// \"action\": () => require(\"//host.lan/Data/Project/javascriptsource/a/b\").Action,",
            "\"action\": () => require(\"//host.lan/Data/Other/javascriptsource/a/b\").Action,",
            "\"value\": () => require(\"https://example.test/source\").Action,",
            "\"action\": () => require(\"../../javascriptsource/a/b.js\").Action,",
        ]
        .join("\n");
        assert_eq!(normalizer.normalize(&untouched, 1).unwrap(), (untouched, 0));
        for suffix in ["../secret", "a/../../secret", "a%2fsecret", "a?x=1", "a\\b"] {
            let source = format!(
                "\"action\": () => require(\"//host.lan/Data/Project/javascriptsource/{suffix}\").Action,"
            );
            assert!(normalizer.normalize(&source, 1).is_err());
        }
    }

    #[test]
    fn watches_fresh_build_regeneration_and_whole_deployment_replacement() {
        let (root, mut normalizer) = fixture();
        assert_eq!(normalizer.scan().unwrap().rewritten_files, 0);
        let project = root.path().join("Project");
        let layout = project.join("deployment/web/layouts/App.js");
        fs::create_dir_all(layout.parent().unwrap()).unwrap();
        fs::write(&layout, ORIGINAL).unwrap();
        let nanoflow = project.join("deployment/web/nanoflows/Atlas_Core.Action.js");
        fs::create_dir_all(nanoflow.parent().unwrap()).unwrap();
        fs::write(&nanoflow, ACTION_ORIGINAL).unwrap();
        fs::create_dir_all(project.join("deployment/web/dist")).unwrap();
        fs::write(project.join("deployment/web/dist/page.js"), ORIGINAL).unwrap();
        fs::write(project.join("model.mpr"), ORIGINAL).unwrap();
        assert_eq!(normalizer.scan().unwrap().pending_files, 2);
        assert_eq!(fs::read_to_string(&layout).unwrap(), ORIGINAL);
        assert_eq!(fs::read_to_string(&nanoflow).unwrap(), ACTION_ORIGINAL);
        assert_eq!(normalizer.scan().unwrap().rewritten_imports, 3);
        assert_eq!(fs::read_to_string(&layout).unwrap(), NORMALIZED);
        assert_eq!(fs::read_to_string(&nanoflow).unwrap(), ACTION_NORMALIZED);
        assert_eq!(normalizer.scan().unwrap().rewritten_files, 0);
        assert_eq!(
            fs::read_to_string(project.join("model.mpr")).unwrap(),
            ORIGINAL
        );
        assert_eq!(
            fs::read_to_string(project.join("deployment/web/dist/page.js")).unwrap(),
            ORIGINAL
        );
        // A real watcher can replace a file, and Clean Deployment can replace
        // the entire tree. Both must be rediscovered using the project anchor.
        fs::write(&layout, ORIGINAL).unwrap();
        fs::write(&nanoflow, ACTION_ORIGINAL).unwrap();
        normalizer.scan().unwrap();
        assert_eq!(normalizer.scan().unwrap().rewritten_files, 2);
        fs::remove_dir_all(project.join("deployment")).unwrap();
        normalizer.scan().unwrap();
        fs::create_dir_all(layout.parent().unwrap()).unwrap();
        fs::create_dir_all(nanoflow.parent().unwrap()).unwrap();
        fs::write(&layout, ORIGINAL).unwrap();
        fs::write(&nanoflow, ACTION_ORIGINAL).unwrap();
        normalizer.scan().unwrap();
        assert_eq!(normalizer.scan().unwrap().rewritten_files, 2);
    }

    #[test]
    fn rejects_link_indirection_and_oversized_files_without_touching_sources() {
        let (root, mut normalizer) = fixture();
        let outside = root.path().join("source.js");
        fs::write(&outside, ORIGINAL).unwrap();
        let web = root.path().join("Project/deployment/web");
        fs::create_dir_all(web.join("layouts")).unwrap();
        let layout = web.join("layouts/App.js");
        symlink(&outside, &layout).unwrap();
        assert!(normalizer.scan().is_err());
        fs::remove_file(&layout).unwrap();
        fs::hard_link(&outside, &layout).unwrap();
        assert!(normalizer.scan().is_err());
        fs::remove_file(&layout).unwrap();
        fs::File::create(&layout)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(normalizer.scan().is_err());
        fs::remove_file(&layout).unwrap();
        fs::remove_dir(web.join("layouts")).unwrap();
        symlink(root.path(), web.join("layouts")).unwrap();
        assert!(normalizer.scan().is_err());
        assert_eq!(fs::read_to_string(outside).unwrap(), ORIGINAL);
        assert!(
            Normalizer::new(root.path(), &root.path().join("Project"), r"\\other\Data").is_err()
        );
    }
}
