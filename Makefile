.PHONY: check-rust-toolchain check-swift-toolchain build-rust test-rust lint-rust build-swift test-swift lint-swift build-all test-all build-app build-app-local-core clean

ifeq ($(OS),Windows_NT)
NULL_DEVICE := NUL
else
NULL_DEVICE := /dev/null
endif

CARGO_VERSION := $(shell cd rust-service && cargo --version 2>$(NULL_DEVICE))
SWIFT_VERSION := $(shell swift --version 2>$(NULL_DEVICE))
VELVT_CODESIGN_IDENTITY ?= E24074F5011AE8FF85C0AD97A583E1CCA6688E81
VELVT_API_BASE_URL ?= https://dev-api.getvelvt.com
VELVT_LOCAL_API_BASE_URL ?= http://localhost:8000

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
	xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' -derivedDataPath $(PWD)/swift-client/DerivedData CONFIGURATION_BUILD_DIR=$(PWD)/swift-client/.build VELVT_API_BASE_URL="$(VELVT_API_BASE_URL)" build

test-swift: check-swift-toolchain
	CLANG_MODULE_CACHE_PATH=$(PWD)/swift-client/.build/clang-module-cache swift test --package-path swift-client --scratch-path $(PWD)/swift-client/.build --disable-sandbox

lint-swift: check-swift-toolchain
	cd swift-client && swift format lint --recursive Sources Tests

build-all: build-rust build-swift

test-all: test-rust test-swift

# Produces a single runnable artifact: dist/velvt-mac.app. The Xcode
# "Bundle Rust Service" Run Script phase builds cargo --release and embeds
# the Rust binary at Contents/Resources/velvt-service automatically.
# ServiceManager (not ServiceProcessLauncher) manages the installed helper
# via SMAppService — no manual binary copy needed here.
build-app: check-swift-toolchain
	rm -rf dist
	mkdir -p dist
	xcodebuild \
		-project swift-client/VelvtMac.xcodeproj \
		-scheme velvt-mac \
		-destination 'platform=macOS' \
		-derivedDataPath dist/.derivedData \
		VELVT_API_BASE_URL="$(VELVT_API_BASE_URL)" \
		build
	cp -R dist/.derivedData/Build/Products/Debug/velvt-mac.app dist/velvt-mac.app
	rm -rf dist/.derivedData
	# Prefer a stable local development identity so macOS TCC can remember
	# Accessibility permission across relaunches and rebuilds. Fall back to
	# ad-hoc signing when the configured identity is not installed locally.
	if ! codesign --force --deep --sign "$(VELVT_CODESIGN_IDENTITY)" dist/velvt-mac.app; then \
		echo "Configured signing identity unavailable; falling back to ad-hoc signing."; \
		codesign --force --deep --sign - dist/velvt-mac.app; \
	fi
	@echo "Built dist/velvt-mac.app"

build-app-local-core: check-swift-toolchain
	rm -rf dist
	mkdir -p dist
	xcodebuild \
		-project swift-client/VelvtMac.xcodeproj \
		-scheme velvt-mac \
		-destination 'platform=macOS' \
		-derivedDataPath dist/.derivedData \
		VELVT_API_BASE_URL="$(VELVT_LOCAL_API_BASE_URL)" \
		build
	cp -R dist/.derivedData/Build/Products/Debug/velvt-mac.app dist/velvt-mac.app
	rm -rf dist/.derivedData
	if ! codesign --force --deep --sign "$(VELVT_CODESIGN_IDENTITY)" dist/velvt-mac.app; then \
		echo "Configured signing identity unavailable; falling back to ad-hoc signing."; \
		codesign --force --deep --sign - dist/velvt-mac.app; \
	fi
	@echo "Built dist/velvt-mac.app with VELVT_API_BASE_URL=$(VELVT_LOCAL_API_BASE_URL)"

clean:
	cd rust-service && cargo clean
	rm -rf swift-client/.build
	rm -rf swift-client/DerivedData
