.PHONY: check-rust-toolchain check-swift-toolchain build-rust test-rust lint-rust build-swift test-swift lint-swift build-all test-all build-app build-local-app build-mvp-app clean

ifeq ($(OS),Windows_NT)
NULL_DEVICE := NUL
else
NULL_DEVICE := /dev/null
endif

CARGO_VERSION := $(shell cd rust-service && cargo --version 2>$(NULL_DEVICE))
SWIFT_VERSION := $(shell swift --version 2>$(NULL_DEVICE))
# Ad-hoc signing is the safe local default.  Set this to a valid Apple
# Development or Developer ID identity only when that identity is installed.
VELVT_CODESIGN_IDENTITY ?= -
LOCAL_API_BASE_URL := http://localhost:8000
MVP_API_BASE_URL := https://dev-api.getvelvt.com
LOCAL_PRODUCT_NAME := Velvt Local
MVP_PRODUCT_NAME := Velvt
LOCAL_BUNDLE_IDENTIFIER := com.velvt.mac.local
MVP_BUNDLE_IDENTIFIER := com.velvt.mac
LOCAL_SOCKET_PATH := ~/.velvt/velvt-local.sock
MVP_SOCKET_PATH := ~/.velvt/velvt-service.sock
LOCAL_DATABASE_PATH := ~/.velvt/velvt-local.sqlite3
MVP_DATABASE_PATH := ~/.velvt/velvt-service.sqlite3

check-rust-toolchain:
ifeq ($(strip $(CARGO_VERSION)),)
	$(error ERROR: Rust toolchain not found. Install the pinned toolchain from rust-service/rust-toolchain.toml and ensure cargo is on PATH)
else
	@echo "$(CARGO_VERSION)"
endif

check-swift-toolchain:
ifeq ($(strip $(SWIFT_VERSION)),)
	$(error ERROR: Swift toolchain not found. Run Swift targets on macOS with Swift 5.10 or later and ensure swift is on PATH)
else
	@echo "$(SWIFT_VERSION)"
endif

build-rust: check-rust-toolchain
	cd rust-service && cargo build --release

test-rust: check-rust-toolchain
	cd rust-service && cargo test

lint-rust: check-rust-toolchain
	cd rust-service && cargo clippy -- -D warnings
	cd rust-service && cargo fmt --check

build-swift: check-swift-toolchain
	xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' -derivedDataPath $(PWD)/swift-client/DerivedData CONFIGURATION_BUILD_DIR=$(PWD)/swift-client/.build VELVT_API_BASE_URL="$(MVP_API_BASE_URL)" build

test-swift: check-swift-toolchain
	CLANG_MODULE_CACHE_PATH=$(PWD)/swift-client/.build/clang-module-cache swift test --package-path swift-client --scratch-path $(PWD)/swift-client/.build --disable-sandbox

lint-swift: check-swift-toolchain
	cd swift-client && swift format lint --recursive Sources Tests

build-all: build-rust build-swift

test-all: test-rust test-swift

## Separate, simultaneously installable artifacts. Each has its own bundle ID,
## Keychain namespace, IPC socket, and local SQLite store.
build-local-app: check-swift-toolchain
	rm -rf "dist/local" "dist/.local-derivedData"
	mkdir -p "dist/local"
	xcodebuild \
		-project swift-client/VelvtMac.xcodeproj \
		-scheme velvt-mac \
		-destination 'platform=macOS' \
		-derivedDataPath "dist/.local-derivedData" \
		VELVT_API_BASE_URL="$(LOCAL_API_BASE_URL)" \
		VELVT_BUNDLE_IDENTIFIER="$(LOCAL_BUNDLE_IDENTIFIER)" \
		VELVT_PRODUCT_NAME="$(LOCAL_PRODUCT_NAME)" \
		VELVT_DISPLAY_NAME="$(LOCAL_PRODUCT_NAME)" \
		VELVT_SOCKET_PATH="$(LOCAL_SOCKET_PATH)" \
		VELVT_DATABASE_PATH="$(LOCAL_DATABASE_PATH)" \
		build
	cp -R "dist/.local-derivedData/Build/Products/Debug/$(LOCAL_PRODUCT_NAME).app" "dist/local/$(LOCAL_PRODUCT_NAME).app"
	rm -rf "dist/.local-derivedData"
	codesign --force --deep --sign "$(VELVT_CODESIGN_IDENTITY)" "dist/local/$(LOCAL_PRODUCT_NAME).app"
	@echo "Built dist/local/$(LOCAL_PRODUCT_NAME).app ($(LOCAL_API_BASE_URL))"

build-mvp-app: check-swift-toolchain
	rm -rf "dist/mvp" "dist/.mvp-derivedData"
	mkdir -p "dist/mvp"
	xcodebuild \
		-project swift-client/VelvtMac.xcodeproj \
		-scheme velvt-mac \
		-destination 'platform=macOS' \
		-derivedDataPath "dist/.mvp-derivedData" \
		VELVT_API_BASE_URL="$(MVP_API_BASE_URL)" \
		VELVT_BUNDLE_IDENTIFIER="$(MVP_BUNDLE_IDENTIFIER)" \
		VELVT_PRODUCT_NAME="$(MVP_PRODUCT_NAME)" \
		VELVT_DISPLAY_NAME="$(MVP_PRODUCT_NAME)" \
		VELVT_SOCKET_PATH="$(MVP_SOCKET_PATH)" \
		VELVT_DATABASE_PATH="$(MVP_DATABASE_PATH)" \
		build
	cp -R "dist/.mvp-derivedData/Build/Products/Debug/$(MVP_PRODUCT_NAME).app" "dist/mvp/$(MVP_PRODUCT_NAME).app"
	rm -rf "dist/.mvp-derivedData"
	codesign --force --deep --sign "$(VELVT_CODESIGN_IDENTITY)" "dist/mvp/$(MVP_PRODUCT_NAME).app"
	@echo "Built dist/mvp/$(MVP_PRODUCT_NAME).app ($(MVP_API_BASE_URL))"

# Backward-compatible name for the deployable dev-API artifact.
build-app: build-mvp-app

clean:
	cd rust-service && cargo clean
	rm -rf swift-client/.build
	rm -rf swift-client/DerivedData
