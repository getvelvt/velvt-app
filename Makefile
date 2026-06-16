.PHONY: check-rust-toolchain check-swift-toolchain build-rust test-rust lint-rust build-swift test-swift lint-swift build-all test-all build-app

ifeq ($(OS),Windows_NT)
NULL_DEVICE := NUL
else
NULL_DEVICE := /dev/null
endif

CARGO_VERSION := $(shell cd rust-service && cargo --version 2>$(NULL_DEVICE))
SWIFT_VERSION := $(shell swift --version 2>$(NULL_DEVICE))

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
	xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' build

test-swift: check-swift-toolchain
	swift test --package-path swift-client

lint-swift: check-swift-toolchain
	cd swift-client && swift format lint --recursive Sources Tests

build-all: build-rust build-swift

test-all: test-rust test-swift

# Produces a single runnable artifact: dist/velvt-mac.app, with the Rust
# service binary embedded at Contents/MacOS/velvt-service. AppDelegate
# launches it at startup (see ServiceProcessLauncher.swift) and terminates
# it on quit, so a real user can install and double-click dist/velvt-mac.app
# without a terminal or any manually-exported environment variables.
build-app: check-rust-toolchain check-swift-toolchain
	cd rust-service && cargo build --release
	rm -rf dist
	mkdir -p dist
	xcodebuild \
		-project swift-client/VelvtMac.xcodeproj \
		-scheme velvt-mac \
		-destination 'platform=macOS' \
		-derivedDataPath dist/.derivedData \
		build
	cp -R dist/.derivedData/Build/Products/Debug/velvt-mac.app dist/velvt-mac.app
	cp rust-service/target/release/velvt-service dist/velvt-mac.app/Contents/MacOS/velvt-service
	rm -rf dist/.derivedData
	@echo "Built dist/velvt-mac.app"
