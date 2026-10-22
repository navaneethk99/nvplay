import unittest
from unittest.mock import patch
import idle


class IdleShutdownTests(unittest.TestCase):
    config = {"host_key": "test-key", "vm_id": "/subscriptions/test/vm"}

    def test_active_session_never_calls_azure(self):
        with patch.object(idle, "request_json", return_value={"active_sessions": 1, "shutdown": False}) as request:
            idle.run(self.config)
        self.assertEqual(request.call_count, 1)

    def test_read_only_check_does_not_claim_shutdown(self):
        with patch.object(idle, "request_json", return_value={"shutdown": False}) as request:
            idle.run(self.config, check=True)
        self.assertEqual(request.call_args.kwargs["method"], "GET")
        self.assertTrue(request.call_args.args[0].endswith("/admin/activity"))

    def test_lease_uses_managed_identity_for_this_vm(self):
        with patch.object(idle, "request_json", side_effect=[{"shutdown": True}, {"access_token": "test-token"}, {}]) as request:
            idle.run(self.config)
        self.assertEqual(request.call_count, 3)
        self.assertEqual(request.call_args.kwargs["method"], "POST")
        self.assertIn(self.config["vm_id"] + "/deallocate?", request.call_args.args[0])
        self.assertEqual(request.call_args.kwargs["headers"]["Authorization"], "Bearer test-token")


if __name__ == "__main__":
    unittest.main()
