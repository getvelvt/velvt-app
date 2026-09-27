.PHONY: check-rust-toolchain check-swift-toolchain check-xcode-version prepare-dmg-tool build-rust test-rust bench-rust check-rust-onnx lint-rust build-swift test-swift lint-swift lint-copy build-all test-all build-app package-release dmg alpha-dmg release update-archive update-appcast verify-update-release test-update-release test-dmg-release test-measurement verify-release verify-release-production build-app-local-core clean

ifeq ($(OS),Windows_NT)
NULL_DEVICE := NUL
else
NULL_DEVICE := /dev/null
endif

CARGO_VERSION := $(shell cd rust-service && cargo --version 2>$(NULL_DEVICE))
SWIFT_VERSION := $(shell swift --version 2>$(NULL_DEVICE))
VELVT_API_BASE_URL ?= https://dev-api.getvelvt.com
VELVT_PRODUCTION_API_BASE_URL ?=
VELVT_APPROVED_PRODUCTION_API_HOST ?=
VELVT_LOCAL_API_BASE_URL ?= http://localhost:8000
MACHINE_ARCH := $(shell uname -m)
VELVT_RELEASE_ARCHS ?= arm64 x86_64
VELVT_APP_PATH ?= dist/Velvt.app
VELVT_DMG_PATH ?= dist/Velvt.dmg
VELVT_APPCAST_PATH ?=
VELVT_UPDATE_ARCHIVE_PATH ?=
VELVT_UPDATE_BASE_URL ?=
VELVT_SPARKLE_BIN_DIR ?=
VELVT_SPARKLE_PRIVATE_KEY_FILE ?=
VELVT_RELEASE_VERSION ?=
VELVT_RELEASE_BUILD ?=
VELVT_PREVIOUS_RELEASE_BUILD ?=
VELVT_UPDATER_ENABLED ?= NO
VELVT_UPDATE_FEED_URL ?=
VELVT_UPDATE_PUBLIC_ED_KEY ?=
# The version and build number live in one file, which the Xcode configurations
# include and rust-service/build.rs reads. Overriding them here is refused by
# alpha-dmg and release (scripts/release_provenance.sh check-release): a version
# that only exists on a command line is how 1.0.0 through 1.0.8 shipped with no
# commit that records their number.
VELVT_VERSION_XCCONFIG := swift-client/Configs/Version.xcconfig
VELVT_BUILD_MARKETING_VERSION ?= $(shell sed -n 's/^[[:space:]]*MARKETING_VERSION[[:space:]]*=[[:space:]]*//p' $(VELVT_VERSION_XCCONFIG) | head -n 1)
VELVT_BUILD_NUMBER ?= $(shell sed -n 's/^[[:space:]]*CURRENT_PROJECT_VERSION[[:space:]]*=[[:space:]]*//p' $(VELVT_VERSION_XCCONFIG) | head -n 1)
# Stamped into Info.plist (VelvtSourceCommit) and the helper (--source-commit).
# Always computed, never taken from the environment or the command line: it is
# evidence, not a setting. Deferred, so only targets that build an app run git.
override VELVT_SOURCE_COMMIT = $(shell ./scripts/release_provenance.sh source-commit)
# package-release and dmg refuse a dirty tree unless this is 1. For local
# experiments only: the build is stamped <commit>-dirty, and alpha-dmg and
# release refuse a dirty tree regardless.
VELVT_ALLOW_DIRTY_TREE ?= 0
VELVT_ALLOW_LOCAL_DMG ?= 0
VELVT_GENERATE_APPCAST_SHA256 ?=
VELVT_SIGN_UPDATE_SHA256 ?=

# ort's downloaded ONNX Runtime binary is available for Apple Silicon, but not
# Intel macOS. The service builds without this optional feature and disables
# Tier 2 embedding classification gracefully on unsupported platforms.
ifeq ($(MACHINE_ARCH),arm64)
CARGO_ONNX_FEATURES := --features onnx
endif

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

# Release builds are made with the Xcode named in .xcode-version, which CI also
# selects, so a shipped binary and the CI run that vetted it share a compiler.
# VELVT_ALLOW_XCODE_MISMATCH=1 builds anyway, for local verification only.
check-xcode-version:
	@expected="Xcode $$(cat .xcode-version)"; \
	actual="$$(xcodebuild -version 2>$(NULL_DEVICE) | head -n 1)"; \
	if [ "$$actual" = "$$expected" ]; then \
		echo "$$actual"; \
	elif [ "$(VELVT_ALLOW_XCODE_MISMATCH)" = "1" ]; then \
		echo "WARNING: building with '$$actual', not the pinned '$$expected'. Do not distribute this build." >&2; \
	else \
		echo "ERROR: release builds use $$expected (.xcode-version); the selected Xcode is '$$actual'." >&2; \
		echo "Install it and select it with sudo xcode-select -s <path to that Xcode.app>, or set VELVT_ALLOW_XCODE_MISMATCH=1 for a local-only build." >&2; \
		exit 1; \
	fi

build-rust: check-rust-toolchain
	cd rust-service && cargo build --release $(CARGO_ONNX_FEATURES)

test-rust: check-rust-toolchain
	cd rust-service && cargo test --workspace

# Where the published Tier 2 p95 budget is measured, and the only place it is:
# the tail bound is #[ignore]d out of test-rust so a busy shared runner cannot
# fail a pull request on a tail sample. The pull request still carries the Tier
# 2 median bound, which runner load does not move. CI runs this target in the
# `bench` job, on pushes to develop and main and on demand -- see .github/workflows/ci.yml,
# which states what that timing trades away.
#
# --release, unlike test-rust, because a wall-clock budget for a shipped binary
# has to be measured on the build that ships. It is not a detail: the builtin
# Tier 2 model measured between seventeen and twenty-six times slower
# unoptimized, depending on input length, and PERFORMANCE_REPORT.md's published
# percentiles were taken in --release -- so a debug measurement is not
# comparable to the number it is being checked against.
#
# CARGO_ONNX_FEATURES, unlike in test-rust, because the real-model tail test is
# behind `#[cfg(feature = "onnx")]` and would otherwise compile nowhere a make
# target reaches. It is empty off Apple Silicon, so CI is unaffected; where it
# is set, the test still returns early unless VELVT_ABSTRACTION_MODEL_PATH and
# VELVT_ABSTRACTION_CENTROIDS_PATH point at real artifacts.
#
# libtest exits 0 when its filter matches nothing, so a renamed or deleted
# benchmark would leave this target green having measured nothing: the same
# unfalsifiable gate the target exists to prevent. Require a non-zero pass
# count. The log file is because a pipe would report tee's exit status and
# swallow a failing benchmark, and /bin/sh on Linux has no pipefail.
BENCH_RUST_LOG := $(CURDIR)/rust-service/target/bench-rust.log

bench-rust: check-rust-toolchain
	@mkdir -p $(dir $(BENCH_RUST_LOG))
	cd rust-service && cargo test --release --workspace $(CARGO_ONNX_FEATURES) -- --ignored --nocapture > $(BENCH_RUST_LOG) 2>&1 || (cat $(BENCH_RUST_LOG) >&2; exit 1)
	@cat $(BENCH_RUST_LOG)
	@grep -qE '^test result: ok\. [1-9][0-9]* passed' $(BENCH_RUST_LOG) || (echo "ERROR: bench-rust measured nothing. The Tier 2 p95 budget published in README.md and ARCHITECTURE.md is the #[ignore]d test in rust-service/tests/embedding_similarity.rs; if it was renamed or removed, that number has no enforcement point." >&2; exit 1)

# src/abstraction/onnx.rs is entirely inside `#[cfg(feature = "onnx")]`, and
# nothing else in this Makefile enables the feature on a machine where
# CARGO_ONNX_FEATURES is empty -- so on any non-arm64 host the module compiles
# nowhere. Type-checking it needs no model artifacts and no ONNX Runtime at
# link time. It does need a platform ort publishes a runtime download for:
# Linux x86_64 (the CI rust job) or Apple Silicon, not Intel macOS.
# --all-targets so the feature-gated tests are type-checked too, since they are
# the only callers of some of the gated surface.
check-rust-onnx: check-rust-toolchain
	cd rust-service && cargo check --features onnx --all-targets

# --workspace --all-targets so shared-types, the integration tests, and every
# #[cfg(test)] module are linted too. Plain `cargo clippy` covers only the root
# package's lib and bin, which let a test-only lint error sit on develop
# unnoticed.
lint-rust: check-rust-toolchain
	cd rust-service && cargo clippy --workspace --all-targets -- -D warnings
	cd rust-service && cargo fmt --all --check

build-swift: check-swift-toolchain
	rm -rf swift-client/.build/velvt-mac.app
	xcodebuild -project swift-client/VelvtMac.xcodeproj -scheme velvt-mac -destination 'generic/platform=macOS' -derivedDataPath $(PWD)/swift-client/DerivedData CONFIGURATION_BUILD_DIR=$(PWD)/swift-client/.build VELVT_API_BASE_URL="$(VELVT_API_BASE_URL)" build

test-swift: check-swift-toolchain
	CLANG_MODULE_CACHE_PATH=$(PWD)/swift-client/.build/clang-module-cache swift test --package-path swift-client --scratch-path $(PWD)/swift-client/.build --disable-sandbox

# A ratchet over swift-format's findings, not a bare `swift format lint`, which
# exits 0 on any number of warnings. See scripts/lint_swift.sh and
# docs/toolchains-and-lint.md.
lint-swift: check-swift-toolchain
	./scripts/lint_swift.sh

# Every shipped Swift and Rust string literal, checked for the capability
# claims the product cannot make: learns, adapts, predicts, gets smarter,
# behavioural modelling. Needs only python3. See scripts/check_banned_copy.py
# for what is scanned, what is not, and the allowlist.
lint-copy:
	./scripts/check_banned_copy.py

build-all: build-rust build-swift

test-all: test-rust test-swift test-measurement

# Produces a single runnable artifact: dist/Velvt.app. The Xcode
# "Bundle Rust Service" Run Script phase builds cargo --release and embeds
# the Rust binary at Contents/Resources/velvt-service automatically.
# ServiceProcessLauncher owns the embedded helper for the app's lifetime; no
# external runtime or manual helper installation is required.
# Backward-compatible release entry point.
build-app: package-release

package-release: check-swift-toolchain check-xcode-version
	@VELVT_ALLOW_DIRTY_TREE="$(VELVT_ALLOW_DIRTY_TREE)" ./scripts/release_provenance.sh require-clean
	./scripts/preflight_distribution.sh "$(VELVT_API_BASE_URL)"
	rm -rf dist/.derivedData-release dist/Velvt.app dist/velvt-mac.app
	rm -f dist/notarization-result.plist
	mkdir -p dist
	xcodebuild \
		-project swift-client/VelvtMac.xcodeproj \
		-scheme velvt-mac \
		-configuration Release \
		-destination 'platform=macOS' \
		-derivedDataPath dist/.derivedData-release \
		ARCHS="$(VELVT_RELEASE_ARCHS)" \
		ONLY_ACTIVE_ARCH=NO \
		VELVT_API_BASE_URL="$(VELVT_API_BASE_URL)" \
		VELVT_UPDATER_ENABLED="$(VELVT_UPDATER_ENABLED)" \
		VELVT_UPDATE_FEED_URL="$(VELVT_UPDATE_FEED_URL)" \
		VELVT_UPDATE_PUBLIC_ED_KEY="$(VELVT_UPDATE_PUBLIC_ED_KEY)" \
		MARKETING_VERSION="$(VELVT_BUILD_MARKETING_VERSION)" \
		CURRENT_PROJECT_VERSION="$(VELVT_BUILD_NUMBER)" \
		VELVT_SOURCE_COMMIT="$(VELVT_SOURCE_COMMIT)" \
		build
	ditto dist/.derivedData-release/Build/Products/Release/Velvt.app dist/Velvt.app
	rm -rf dist/.derivedData-release
	./scripts/sign_release.sh local $(VELVT_APP_PATH)
	VELVT_RELEASE_ARCHS="$(VELVT_RELEASE_ARCHS)" ./scripts/verify_release.sh --mode local --app $(VELVT_APP_PATH)
	@echo "Built and verified Release artifact dist/Velvt.app"

prepare-dmg-tool:
	./scripts/prepare_dmg_tool.sh

dmg: check-swift-toolchain
	@test "$(VELVT_ALLOW_LOCAL_DMG)" = "1" || (echo "ERROR: 'make dmg' creates an ad-hoc signed, build-machine-only image." >&2; echo "Use 'make alpha-dmg' for testers, or set VELVT_ALLOW_LOCAL_DMG=1 for local verification only." >&2; exit 1)
	@$(MAKE) package-release
	@$(MAKE) prepare-dmg-tool
	./scripts/create_dmg.sh $(VELVT_APP_PATH) $(VELVT_DMG_PATH) local
	VELVT_RELEASE_ARCHS="$(VELVT_RELEASE_ARCHS)" ./scripts/verify_release.sh --mode local --app $(VELVT_APP_PATH) --dmg $(VELVT_DMG_PATH)

# Signed, notarized, stapled DMG that installs on a Mac which has never built
# Velvt. `make dmg` cannot do this: it is ad-hoc signed, so under quarantine
# library validation kills the nested helper and Sparkle at load and the user
# sees a crash dialog rather than a Gatekeeper prompt.
#
# This is the gap between `dmg` and `release`. It deliberately omits Sparkle
# appcast and archive generation -- whether a tester can install the app at all
# is a separate question from whether it can update itself -- so it needs two
# credentials instead of the seventeen variables `release` demands.
#
#   make alpha-dmg \
#     VELVT_CODESIGN_IDENTITY="Developer ID Application: NAME (TEAMID)" \
#     VELVT_NOTARY_PROFILE=VELVT_NOTARY \
#     VELVT_DMG_PATH=dist/Velvt-1.0.12.dmg
#
# See docs/shipping-a-testable-dmg.md for obtaining both. It builds the version
# in swift-client/Configs/Version.xcconfig from a clean commit, refuses a
# version or build that already shipped (docs/RELEASES.md), and tags the commit
# locally once the DMG is notarized and verified.
alpha-dmg: check-swift-toolchain
	@./scripts/release_provenance.sh check-release "$(VELVT_BUILD_MARKETING_VERSION)" "$(VELVT_BUILD_NUMBER)"
	@$(MAKE) prepare-dmg-tool
	@test -n "$(VELVT_CODESIGN_IDENTITY)" || (echo "ERROR: set VELVT_CODESIGN_IDENTITY to a 'Developer ID Application: NAME (TEAMID)' identity. See docs/shipping-a-testable-dmg.md." >&2; exit 1)
	@test -n "$(VELVT_NOTARY_PROFILE)" || (echo "ERROR: set VELVT_NOTARY_PROFILE to a notarytool Keychain profile. See docs/shipping-a-testable-dmg.md." >&2; exit 1)
	@test ! -e "$(VELVT_DMG_PATH)" && test ! -e "$(VELVT_DMG_PATH).sha256" || (echo "ERROR: $(VELVT_DMG_PATH) already exists; choose a new versioned VELVT_DMG_PATH." >&2; exit 1)
	$(MAKE) package-release
	./scripts/sign_release.sh production $(VELVT_APP_PATH)
	./scripts/notarize_app.sh $(VELVT_APP_PATH)
	./scripts/create_dmg.sh $(VELVT_APP_PATH) $(VELVT_DMG_PATH) production
	codesign --force --sign "$(VELVT_CODESIGN_IDENTITY)" --timestamp $(VELVT_DMG_PATH)
	./scripts/notarize_release.sh $(VELVT_DMG_PATH)
	# Recorded as a bare basename, matching create_dmg.sh's local-mode
	# checksum and what verify_release.sh expects: the verifier cd's into
	# the DMG's directory before running `shasum -c`, so a path-qualified
	# entry here resolves to dist/dist/... and fails to open.
	cd $(dir $(VELVT_DMG_PATH)) && shasum -a 256 $(notdir $(VELVT_DMG_PATH)) > $(notdir $(VELVT_DMG_PATH)).sha256
	$(MAKE) verify-release-production
	./scripts/release_provenance.sh tag "$(VELVT_BUILD_MARKETING_VERSION)" "$(VELVT_BUILD_NUMBER)" $(VELVT_DMG_PATH)
	@echo ""
	@echo "Notarized alpha DMG ready: $(VELVT_DMG_PATH)"
	@echo "Verify on a Mac that has never built Velvt, downloaded via a browser"
	@echo "so it carries a real quarantine flag. Do not run xattr on it."

# Production is intentionally credential-gated. It never falls back to ad-hoc
# signing and only succeeds after Apple accepts and the DMG is stapled.
release: check-swift-toolchain
	@test -n "$(VELVT_RELEASE_VERSION)" || (echo "ERROR: set VELVT_RELEASE_VERSION to CFBundleShortVersionString." >&2; exit 1)
	@test -n "$(VELVT_RELEASE_BUILD)" || (echo "ERROR: set VELVT_RELEASE_BUILD to CFBundleVersion." >&2; exit 1)
	@./scripts/release_provenance.sh check-release "$(VELVT_RELEASE_VERSION)" "$(VELVT_RELEASE_BUILD)"
	@$(MAKE) prepare-dmg-tool
	@test -n "$(VELVT_PRODUCTION_API_BASE_URL)" || (echo "ERROR: set VELVT_PRODUCTION_API_BASE_URL to the approved production endpoint." >&2; exit 1)
	@test -n "$(VELVT_APPROVED_PRODUCTION_API_HOST)" || (echo "ERROR: set VELVT_APPROVED_PRODUCTION_API_HOST to the separately approved exact hostname." >&2; exit 1)
	@./scripts/preflight_distribution.sh "$(VELVT_PRODUCTION_API_BASE_URL)" production "$(VELVT_APPROVED_PRODUCTION_API_HOST)"
	@test -n "$(VELVT_CODESIGN_IDENTITY)" || (echo "ERROR: set VELVT_CODESIGN_IDENTITY to a Developer ID Application identity." >&2; exit 1)
	@test -n "$(VELVT_NOTARY_PROFILE)" || (echo "ERROR: set VELVT_NOTARY_PROFILE to a notarytool Keychain profile." >&2; exit 1)
	@test -n "$(VELVT_APPCAST_PATH)" || (echo "ERROR: set VELVT_APPCAST_PATH to the signed production appcast." >&2; exit 1)
	@test -n "$(VELVT_UPDATE_ARCHIVE_PATH)" || (echo "ERROR: set VELVT_UPDATE_ARCHIVE_PATH to a new, versioned .zip path." >&2; exit 1)
	@test -n "$(VELVT_UPDATE_BASE_URL)" || (echo "ERROR: set VELVT_UPDATE_BASE_URL to the HTTPS archive base URL." >&2; exit 1)
	@test -n "$(VELVT_SPARKLE_BIN_DIR)" || (echo "ERROR: set VELVT_SPARKLE_BIN_DIR to Sparkle 2.9.4's bin directory." >&2; exit 1)
	@test -n "$(VELVT_SPARKLE_PRIVATE_KEY_FILE)" || (echo "ERROR: set VELVT_SPARKLE_PRIVATE_KEY_FILE to an external mode-0600 Ed25519 private key." >&2; exit 1)
	@test -n "$(VELVT_RELEASE_VERSION)" || (echo "ERROR: set VELVT_RELEASE_VERSION to CFBundleShortVersionString." >&2; exit 1)
	@test -n "$(VELVT_RELEASE_BUILD)" || (echo "ERROR: set VELVT_RELEASE_BUILD to CFBundleVersion." >&2; exit 1)
	@test -n "$(VELVT_PREVIOUS_RELEASE_BUILD)" || (echo "ERROR: set VELVT_PREVIOUS_RELEASE_BUILD to the installed N build." >&2; exit 1)
	@test -n "$(VELVT_UPDATE_FEED_URL)" || (echo "ERROR: set VELVT_UPDATE_FEED_URL to the production HTTPS appcast URL." >&2; exit 1)
	@test -n "$(VELVT_UPDATE_PUBLIC_ED_KEY)" || (echo "ERROR: set VELVT_UPDATE_PUBLIC_ED_KEY to the Sparkle Ed25519 public key." >&2; exit 1)
	@test -n "$(VELVT_GENERATE_APPCAST_SHA256)" || (echo "ERROR: set VELVT_GENERATE_APPCAST_SHA256 to the approved Sparkle 2.9.4 tool checksum." >&2; exit 1)
	@test -n "$(VELVT_SIGN_UPDATE_SHA256)" || (echo "ERROR: set VELVT_SIGN_UPDATE_SHA256 to the approved Sparkle 2.9.4 tool checksum." >&2; exit 1)
	@test ! -e "$(VELVT_DMG_PATH)" && test ! -e "$(VELVT_DMG_PATH).sha256" || (echo "ERROR: production DMG outputs are immutable; choose a new versioned VELVT_DMG_PATH." >&2; exit 1)
	$(MAKE) package-release VELVT_API_BASE_URL="$(VELVT_PRODUCTION_API_BASE_URL)" VELVT_UPDATER_ENABLED=YES VELVT_UPDATE_FEED_URL="$(VELVT_UPDATE_FEED_URL)" VELVT_UPDATE_PUBLIC_ED_KEY="$(VELVT_UPDATE_PUBLIC_ED_KEY)" VELVT_BUILD_MARKETING_VERSION="$(VELVT_RELEASE_VERSION)" VELVT_BUILD_NUMBER="$(VELVT_RELEASE_BUILD)"
	./scripts/sign_release.sh production $(VELVT_APP_PATH)
	./scripts/notarize_app.sh $(VELVT_APP_PATH)
	./scripts/create_dmg.sh $(VELVT_APP_PATH) $(VELVT_DMG_PATH) production
	codesign --force --sign "$(VELVT_CODESIGN_IDENTITY)" --timestamp $(VELVT_DMG_PATH)
	./scripts/notarize_release.sh $(VELVT_DMG_PATH)
	# Recorded as a bare basename, matching create_dmg.sh's local-mode
	# checksum and what verify_release.sh expects: the verifier cd's into
	# the DMG's directory before running `shasum -c`, so a path-qualified
	# entry here resolves to dist/dist/... and fails to open.
	cd $(dir $(VELVT_DMG_PATH)) && shasum -a 256 $(notdir $(VELVT_DMG_PATH)) > $(notdir $(VELVT_DMG_PATH)).sha256
	$(MAKE) verify-release-production
	$(MAKE) update-archive
	$(MAKE) update-appcast
	$(MAKE) verify-update-release
	./scripts/release_provenance.sh tag "$(VELVT_RELEASE_VERSION)" "$(VELVT_RELEASE_BUILD)" $(VELVT_DMG_PATH)

update-archive:
	./scripts/create_update_archive.sh \
		--app "$(VELVT_APP_PATH)" \
		--archive "$(VELVT_UPDATE_ARCHIVE_PATH)" \
		--expected-build "$(VELVT_RELEASE_BUILD)" \
		--expected-version "$(VELVT_RELEASE_VERSION)"

update-appcast:
	./scripts/generate_update_appcast.sh \
		--archive "$(VELVT_UPDATE_ARCHIVE_PATH)" \
		--appcast "$(VELVT_APPCAST_PATH)" \
		--download-url-prefix "$(VELVT_UPDATE_BASE_URL)" \
		--private-key-file "$(VELVT_SPARKLE_PRIVATE_KEY_FILE)" \
		--sparkle-bin-dir "$(VELVT_SPARKLE_BIN_DIR)" \
		--expected-build "$(VELVT_RELEASE_BUILD)" \
		--expected-generate-appcast-sha256 "$(VELVT_GENERATE_APPCAST_SHA256)" \
		--expected-sign-update-sha256 "$(VELVT_SIGN_UPDATE_SHA256)"

verify-update-release:
	./scripts/verify_update_readiness.sh \
		--app "$(VELVT_APP_PATH)" \
		--appcast "$(VELVT_APPCAST_PATH)" \
		--archive "$(VELVT_UPDATE_ARCHIVE_PATH)" \
		--previous-build "$(VELVT_PREVIOUS_RELEASE_BUILD)" \
		--expected-update-version "$(VELVT_RELEASE_BUILD)"

test-update-release:
	./scripts/tests/release_provenance_test.sh
	./scripts/tests/verify_update_readiness_test.sh
	./scripts/tests/update_release_scripts_test.sh
	./scripts/tests/update_local_adversarial_harness_test.sh
	./scripts/tests/release_policy_test.sh

test-dmg-release:
	./scripts/tests/dmg_release_policy_test.sh

# The measurement instrument: the cohort export, the analysis harness, the
# boundary proof, and the synthetic trace generator. These guard the numbers the
# pre-registration will be reported from, so they run with the rest of the suite
# rather than only when a human remembers.
test-measurement:
	./scripts/tests/run_measurement_tests.sh

verify-release:
	VELVT_RELEASE_ARCHS="$(VELVT_RELEASE_ARCHS)" ./scripts/verify_release.sh --mode local --app $(VELVT_APP_PATH) --dmg $(VELVT_DMG_PATH)

verify-release-production:
	VELVT_RELEASE_ARCHS="$(VELVT_RELEASE_ARCHS)" ./scripts/verify_release.sh --mode production --app $(VELVT_APP_PATH) --dmg $(VELVT_DMG_PATH)

build-app-local-core: check-swift-toolchain
	rm -rf dist/.derivedData-local dist/Velvt.app dist/velvt-mac.app
	mkdir -p dist
	xcodebuild \
		-project swift-client/VelvtMac.xcodeproj \
		-scheme velvt-mac \
		-configuration Debug \
		-destination 'platform=macOS' \
		-derivedDataPath dist/.derivedData-local \
		VELVT_API_BASE_URL="$(VELVT_LOCAL_API_BASE_URL)" \
		build
	ditto dist/.derivedData-local/Build/Products/Debug/Velvt.app dist/Velvt.app
	rm -rf dist/.derivedData-local
	@# Silently downgrading to ad-hoc produced an artifact that ran on the build
	@# machine and crashed everywhere else. Ad-hoc is now opt-in: pass
	@# VELVT_ALLOW_ADHOC=1 (or VELVT_CODESIGN_IDENTITY=-) to accept a
	@# build-machine-only bundle.
	if ! codesign --force --deep --sign "$(VELVT_CODESIGN_IDENTITY)" dist/Velvt.app; then \
		if [ "$(VELVT_ALLOW_ADHOC)" = "1" ]; then \
			echo "Signing identity unavailable; ad-hoc signing because VELVT_ALLOW_ADHOC=1."; \
			echo "This bundle runs on this machine only and must never be distributed."; \
			codesign --force --deep --sign - dist/Velvt.app; \
		else \
			echo "ERROR: signing identity '$(VELVT_CODESIGN_IDENTITY)' is unavailable." >&2; \
			echo "Set VELVT_ALLOW_ADHOC=1 to accept a build-machine-only ad-hoc bundle," >&2; \
			echo "or use 'make alpha-dmg' for something a tester can install." >&2; \
			exit 1; \
		fi; \
	fi
	@echo "Built dist/Velvt.app with VELVT_API_BASE_URL=$(VELVT_LOCAL_API_BASE_URL)"

clean:
	cd rust-service && cargo clean
	rm -rf swift-client/.build
	rm -rf swift-client/DerivedData
