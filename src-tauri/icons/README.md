App icons go here. Generate them from a source image with:

```sh
cargo tauri icon path/to/logo.png
```

This produces `32x32.png`, `128x128.png`, `128x128@2x.png`, `icon.icns`,
`icon.ico`, and the platform variants referenced by `../tauri.conf.json`.
They are intentionally not committed (binary, regenerable).
