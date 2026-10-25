import unittest
from scope import select

class ScopeTests(unittest.TestCase):
    def test_docs_do_not_compile(self):
        self.assertFalse(any(select(["README.md"]).values()))
    def test_server_changes_do_not_compile_installer(self):
        self.assertEqual(select(["updates/services/update-server/src/main.rs"]), dict(ui=False, native=False, updates=True))
    def test_shared_client_changes_reach_installer(self):
        self.assertTrue(select(["updates/crates/update-client/src/lib.rs"])["native"])
    def test_product_builder_changes_reach_ui_and_native(self):
        self.assertEqual(select(["scripts/product.ts"]), dict(ui=True, native=True, updates=False))
    def test_unknown_changes_fail_broad(self):
        self.assertTrue(all(select(["new-component/config.json"]).values()))

if __name__ == "__main__":
    unittest.main()
