use std::sync::atomic::{AtomicBool, Ordering};

static DESKTOP_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) fn enable_desktop() {
    DESKTOP_ACTIVE.store(true, Ordering::Release);
}

pub(crate) fn enabled() -> bool {
    cfg!(debug_assertions)
        && DESKTOP_ACTIVE.load(Ordering::Acquire)
        && std::env::var("MENDIMARU_STUDIO_TRACE").as_deref() == Ok("1")
}
