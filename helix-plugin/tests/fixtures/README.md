# Compatibility components

Version-suffixed `.wasm` files are frozen ABI fixtures. `build-plugins.sh`
rebuilds the unsuffixed test components and live examples, and never overwrites
these snapshots.

- `text-tools-v0.1.wasm`: original API 0.1 compatibility component.
- `testplugin2-v0.2.wasm`: API 0.2 test plugin, including the M11 regression
  commands, frozen before API 0.3 work.
- `text-tools-v0.2.wasm`: API 0.2 example, frozen before API 0.3 work.
- `testplugin3-v0.3.wasm`: API 0.3 test plugin, frozen before API 0.4.
- `oil-v0.3.wasm`: the API 0.3 build of `examples/oil`, frozen before 0.4.
- `testplugin3-v0.4.wasm`: API 0.4 test plugin, frozen before the
  `requests`, `revisions` and `snippets` imports were added to 0.4.
- `testplugin3-v0.4.1.wasm`: the last API 0.4 build of the test plugin
  (with `requests`, `revisions`, `snippets`, `panels` and `listener`),
  frozen when the SDK moved to API 0.5.

Host and in-editor API 0.2 tests use the frozen components. Rebuilding a current
SDK or guest must not change the evidence that previously built plugins load
and run.

SHA-256 of the API 0.2 snapshots:

```
c89a8bd6f17844280b79ae44b4997555013ba2ce18964cd1b50557e8a104739c  testplugin2-v0.2.wasm
d221951de450f7900e7e60cfa6bafd3d40e36ee7f5c8d4ba45e399f76ac7335a  text-tools-v0.2.wasm
```

SHA-256 of the API 0.3 snapshots:

```
bcda781832f3fcbc0da8d7ea47add9f8ae717becd91fad95cb93bc9ae217bd84  testplugin3-v0.3.wasm
ba52282acbd9509dc70bbe34631ac307c01d5edc766fd40aeff68f9012dad5e4  oil-v0.3.wasm
```

SHA-256 of the API 0.4 snapshots:

```
4f26c1c0b7673e38c82358557405cc01eed89524ac5cf6015927c99b72afdec1  testplugin3-v0.4.wasm
c9e1e70cca1a1b91566f801bf0a1e1173d805066deef663a96df0cda1fbb36f3  testplugin3-v0.4.1.wasm
```
