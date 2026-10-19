"""Read-only authentication acceptance; prints no headers, bodies or exceptions."""
import argparse
import base64
import json
import os
import secrets
import unicodedata
from urllib.error import HTTPError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def verify(origin, password, shutdown=False):
    target = urlsplit(origin)
    if (target.scheme != "http" or target.hostname != "127.0.0.1" or target.port != 18090
            or target.username or target.password or target.path not in ("", "/")
            or target.query or target.fragment):
        raise ValueError("Expected the fixed loopback admin origin")
    if not password or len(password.encode("utf-8")) > 256 or any(unicodedata.category(c) == "Cc" for c in password):
        raise ValueError("Invalid environment value")
    opener = build_opener(ProxyHandler({}), NoRedirect())
    encode = lambda value: base64.b64encode(value.encode("utf-8")).decode("ascii")
    credentials = [None, encode("incorrect-" + secrets.token_hex(32)), encode(password)]

    def post(path, payload, credential):
        headers = {"Content-Type": "application/json", "Connection": "close"}
        if credential is not None:
            headers["X-M2EE-Authentication"] = credential
        request = Request(origin.rstrip("/") + path, json.dumps(payload).encode(), headers, method="POST")
        try:
            response = opener.open(request, timeout=20)
        except HTTPError as error:
            response = error
        with response:
            body = response.read(1048577)
            if len(body) > 1048576:
                raise ValueError("Response exceeds bound")
            return response.code, body

    servlet = []
    for credential in credentials:
        code, body = post("/dev/preview_execute_oql", {
            "oql": "SELECT l.Code AS Code FROM System.Language l", "numberHandling": "asString",
        }, credential)
        servlet.append(code)
    if servlet[0] not in (401, 403) or servlet[1] not in (401, 403) or servlet[2] != 200:
        raise ValueError("Development servlet authentication failed")
    data = json.loads(body)
    if not isinstance(data, dict) or "error" in data or "data" not in data or data["data"] is None:
        raise ValueError("OQL did not return successful data")
    results = []
    for index, credential in enumerate(credentials):
        code, body = post("/", {"action": "runtime_status", "params": {}}, credential)
        if index < 2 and code in (401, 403):
            results.append({"httpStatus": code, "result": None})
            continue
        payload = json.loads(body)
        result = payload.get("result") if isinstance(payload, dict) else None
        if index < 2:
            if not (code in (401, 403) or (code == 200 and type(result) is int and result == -4)):
                raise ValueError("M2EE accepted an unauthorized request")
        elif code != 200 or type(result) is not int or result != 0:
            raise ValueError("M2EE runtime_status failed")
        results.append({"httpStatus": code, "result": result})
    report = {"developmentServlet": {"missing": servlet[0], "incorrect": servlet[1], "valid": servlet[2]}, "m2eeRuntimeStatus": results}
    if shutdown:
        code, body = post("/", {"action": "shutdown", "params": {}}, credentials[2])
        payload = json.loads(body)
        if code != 200 or type(payload.get("result")) is not int or payload["result"] != 0:
            raise ValueError("M2EE shutdown failed")
        report["m2eeShutdown"] = {"httpStatus": code, "result": 0}
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--admin-origin", default="http://127.0.0.1:18090")
    parser.add_argument("--password-env", default="MXCLI_APP_ADMIN_PASSWORD")
    parser.add_argument("--shutdown", action="store_true", help="Also request M2EE shutdown; verify Studio Stop and owned session/port cleanup separately")
    args = parser.parse_args()
    try:
        report = verify(args.admin_origin, os.environ.get(args.password_env, ""), args.shutdown)
    except Exception:
        parser.exit(1, "FAIL: Studio M2EE authentication verification failed\n")
    print(json.dumps({"ok": True, **report}))


if __name__ == "__main__":
    main()
