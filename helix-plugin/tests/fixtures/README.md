# Compatibility components

Version-suffixed `.wasm` files are frozen ABI fixtures. `build-plugins.sh`
rebuilds the unsuffixed test components and live examples, and never overwrites
these snapshots.

- `text-tools-v0.1.wasm`: original API 0.1 compatibility component.
- `testplugin2-v0.2.wasm`: API 0.2 test plugin, including the M11 regression
  commands, frozen before API 0.3 work.
- `text-tools-v0.2.wasm`: API 0.2 example, frozen before API 0.3 work.

Host and in-editor API 0.2 tests use the frozen components. Rebuilding a current
SDK or guest must not change the evidence that previously built plugins load
and run.

SHA-256 of the API 0.2 snapshots:

```
c89a8bd6f17844280b79ae44b4997555013ba2ce18964cd1b50557e8a104739c  testplugin2-v0.2.wasm
d221951de450f7900e7e60cfa6bafd3d40e36ee7f5c8d4ba45e399f76ac7335a  text-tools-v0.2.wasm
```
