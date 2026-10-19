import importlib.util
import io
import json
import unittest
from pathlib import Path
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("verify_m2ee", Path(__file__).with_name("verify-studio-m2ee.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class Response(io.BytesIO):
    def __init__(self, code, payload):
        super().__init__(json.dumps(payload).encode())
        self.code = code


class Verification(unittest.TestCase):
    def run_case(self, responses, shutdown=False):
        class Opener:
            def open(self, request, timeout):
                self.request = request
                return responses.pop(0)
        with patch.object(module, "build_opener", return_value=Opener()):
            return module.verify("http://127.0.0.1:18090", "test-only-한글-é", shutdown)

    def good(self):
        return [Response(403, {}), Response(403, {}), Response(200, {"data": []}),
                Response(200, {"result": -4}), Response(200, {"result": -4}), Response(200, {"result": 0})]

    def test_successful_authentication_and_shutdown(self):
        result = self.run_case(self.good() + [Response(200, {"result": 0})], True)
        self.assertEqual(result["m2eeShutdown"]["result"], 0)
        self.assertEqual(result["developmentServlet"], {"missing": 403, "incorrect": 403, "valid": 200})

    def test_http_authentication_rejection_needs_no_json_body(self):
        responses = self.good()
        denied = io.BytesIO(b"Forbidden")
        denied.code = 403
        responses[3] = denied
        self.assertEqual(self.run_case(responses)["m2eeRuntimeStatus"][0]["httpStatus"], 403)

    def test_http_200_authentication_denial_is_failure(self):
        cases = [(-4, 200), (None, 200), (False, 200), ("0", 200), (0, 403)]
        for result, code in cases:
            with self.subTest(result=result, code=code):
                responses = self.good()
                responses[-1] = Response(code, {"result": result})
                with self.assertRaises(ValueError):
                    self.run_case(responses)
        with self.assertRaises(ValueError):
            self.run_case(self.good() + [Response(200, {"result": -4})], True)

    def test_unauthenticated_success_and_oql_error_are_failures(self):
        for index, replacement in [(0, Response(200, {})), (2, Response(200, {"error": "private"})), (3, Response(200, {"result": 0}))]:
            responses = self.good()
            responses[index] = replacement
            with self.assertRaises(ValueError):
                self.run_case(responses)

    def test_only_fixed_loopback_origin_and_nonempty_password(self):
        for origin in ["http://example.com:18090", "http://127.0.0.1:8090", "http://user@127.0.0.1:18090", "http://127.0.0.1:18090/?secret"]:
            with self.assertRaises(ValueError):
                module.verify(origin, "fixture")
        for password in ["", "a\nb", "a\u0085b", "x"*257]:
            with self.assertRaises(ValueError):
                module.verify("http://127.0.0.1:18090", password)


if __name__ == "__main__":
    unittest.main()
