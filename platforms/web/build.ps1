cargo build -p app-web --target wasm32-unknown-unknown --release
wasm-bindgen target/wasm32-unknown-unknown/release/app_web.wasm --out-dir platforms/web/pkg --target web
python -m http.server 8080 --directory platforms/web
