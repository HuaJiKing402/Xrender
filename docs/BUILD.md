# Build

## 通用检查（本机已验证）

```bash
cargo metadata --format-version 1 > /dev/null
cargo check -p renderer-core -p app-desktop
cargo test -p renderer-core
cargo fmt --all -- --check
cargo clippy -p renderer-core -p app-desktop -- -D warnings
cargo doc -p renderer-core -p app-desktop --no-deps
```

## Windows / Linux 桌面

```bash
cargo run -p app-desktop                            # 内置三角形
MAX_FRAMES=5 cargo run -p app-desktop               # 无交互验证
MODEL_PATH=models/cube.obj MAX_FRAMES=5 cargo run -p app-desktop
MODEL_PATH=models/triangle.gltf MAX_FRAMES=5 cargo run -p app-desktop
cargo run -p app-desktop -- path/to/your_model.glb
```

支持 OBJ / glTF / GLB。模型自动居中缩放；`TRIANGLE_MAX_FRAMES` 作为旧环境变量仍兼容。

桌面强制 Vulkan。构建不需要 Vulkan SDK，但**运行需要可用的 Vulkan 驱动**；若没有，程序会打印明确诊断并退出。

## Web / WASM（脚手架，未在本机验证）

```bash
rustup target add wasm32-unknown-unknown
./platforms/web/build.sh       # 或 PowerShell: .\platforms\web\build.ps1
```

脚本构建 `app-web`、运行 `wasm-bindgen` 到 `platforms/web/pkg/`，然后启动静态服务器。浏览器需要支持 WebGPU；首版不做 WebGL2 降级。

## Android（脚手架，未在本机验证）

```bash
cargo ndk -t arm64-v8a -p 26 -o platforms/android/app/src/main/jniLibs \
  build -p app-android --release
cd platforms/android && ./gradlew assembleDebug
```

要求 NDK、minSdk 26、设备支持 Vulkan。`AndroidManifest.xml` 与 Gradle 骨架在 `platforms/android/`。

## macOS / iOS（脚手架，未在本机验证）

```bash
cd platforms/apple
./build.sh
```

使用 `xcodegen` 生成工程后 `xcodebuild`。入口 crate 为 `app-apple`，后端为 Metal。iOS 的 surface 生命周期与 Xcode 签名需按实际设备配置。
