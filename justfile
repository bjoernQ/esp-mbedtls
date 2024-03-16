export SSID := "Dummy"
export PASSWORD := "Dummy"

all: (check "esp32" "esp") (check "esp32s3" "esp") (check "esp32c3" "nightly")
    cd esp-mbedtls && cargo +nightly fmt --all -- --check
    
[private]
check arch toolchain:
    cargo +{{ toolchain }} b{{ arch }} --example sync_client
    cargo +{{ toolchain }} b{{ arch }} --example sync_client_mTLS
    cargo +{{ toolchain }} b{{ arch }} --example async_client --features="async"
    cargo +{{ toolchain }} b{{ arch }} --example async_client_mTLS --features="async"
    cargo +{{ toolchain }} b{{ arch }} --example sync_server
    cargo +{{ toolchain }} b{{ arch }} --example sync_server_mTLS
    cargo +{{ toolchain }} b{{ arch }} --example async_server --features="async"
    cargo +{{ toolchain }} b{{ arch }} --example async_server_mTLS --features="async"
    cargo +{{ toolchain }} fmt --all -- --check
