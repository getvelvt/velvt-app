# Release ledger

Every notarized Velvt build that has been released, and the source it came
from, as far as that source can still be identified. Ten builds have been
notarized and released: 1.0.0, 1.0.1, 1.0.3, 1.0.4, 1.0.6, 1.0.7, 1.0.8,
1.0.9, 1.0.10 and 1.0.11. There was no 1.0.2 and no 1.0.5.

Source tags exist for **v1.0.0, v1.0.1, v1.0.9 and v1.0.11 only**. The source
of 1.0.10 was not preserved, and the source of 1.0.3 through 1.0.7 was never
recorded. No release has been shown to rebuild byte for byte from its tag, so
this ledger does not claim that any release is reproducible.

**Recipients, all releases: the founder's own Macs only.** There are two
installs, both on the founder's Macs, and no external installs. No
testing-alpha invitation has been sent. 1.0.0 and 1.0.1 were also published as
assets of public GitHub releases; on 2026-09-25 GitHub counted two downloads of
each DMG, and it does not record who downloaded them.

## Releases

Newest first. `scripts/release_provenance.sh` reads the first two columns
(version, build) of this table, so keep one row per release in this shape.

| Version | Build | IPC protocol | Migrations | DMG built | Source | DMG sha256 | Notarization submission IDs |
|---|---:|---:|---|---|---|---|---|
| 1.0.11 | 17 | 30 | 0001–0036 | 2026-09-24 | tag `v1.0.11` (`2b88c9a`). Built from an uncommitted working tree; that exact tree was committed and tagged on 2026-09-25 | `3288dc4e3a32242ee4a3b174c250e2823b4a97e16867c076be9bf7493a1c4e4f` | app `7c111145-a67a-4dc7-8871-3d8274ad77a6`, DMG `8329fd8d-db66-45ad-99c8-3118ea0402ae` |
| 1.0.10 | 16 | 29 | 0001–0032 | 2026-09-23 | **source not preserved.** Built from an uncommitted working tree that no commit captured; untagged | `22cf19cf4253d77fe7dbfbe34afc61a4de9cad886454ce41c866f23ec33be54a` | not recorded |
| 1.0.9 | 15 | 28 | 0001–0031 | 2026-09-20 | tag `v1.0.9` (`6a963d7`) | `7d0f761ce2717a2b0ab4ad6ce65debb851f68902b6a8d6d3bd0ddb76ce0f2ec8` | not recorded |
| 1.0.8 | 14 | 28 | 0001–0031 | 2026-09-03 | `707c001`, an unreviewed commit (kept as tag `archive/unreviewed-remediation-2026-09-02`). The DMG embeds migration 0031, which on 2026-09-03 existed in no other commit; `develop` received it on 2026-09-14. Untagged as a release | `a66c0d6d44583b7dba1804e3ca58ef2b61302110628a81e9532a7cbfbaf1ac71` | not recorded |
| 1.0.7 | 13 | 28 | 0001–0029 | 2026-08-27 | not identified: untagged, and the version came from the `make` command line, so no commit records it | `23a57e9f6f97b16b4f7dd8271050c7c48ce928fad3cddc20424df94050880ed5` | not recorded |
| 1.0.6 | 12 | 28 | 0001–0029 | 2026-08-26 | not identified (as 1.0.7) | `bfaf80aa02b8fd851663b5c2c87d78691725145e7f792e3b02d732af65823f70` | not recorded |
| 1.0.4 | 10 | 28 | 0001–0029 | 2026-08-23 | not identified (as 1.0.7) | `378a30cb2b1906f3c85553aa7f0abe5828451274c71541bf257e61409d64e8c6` | not recorded |
| 1.0.3 | 9 | 28 | 0001–0029 | 2026-08-22 | not identified (as 1.0.7) | `90cdc92467880fca0a33cc583d9e546df86348e95563a708d135a0191a191e9d` | not recorded |
| 1.0.1 | 6 | 25 | 0001–0017 | 2026-08-17 | tag `v1.0.1` (`2d9d710`). The tag's `Release.xcconfig` says 1.0.0 (2): the version and build came from the `make` command line | `8ea38cdb53b5856c04221371276b2544f9a06c0c97bde71a0310420fbdffdcc6` | not recorded |
| 1.0.0 | 5 | 25 | 0001–0016 | 2026-08-14 | tag `v1.0.0` (`2a08163`). As 1.0.1, the tag's `Release.xcconfig` says 1.0.0 (2) | `9e2003bf86263bfcd11d140b58dd63d0abd6bd47027c949524467f6001e1ec5c` | not recorded |

Where each value comes from:

- **1.0.3 to 1.0.11**: version, build, protocol and API URL were read from the
  `Info.plist` of each DMG, mounted read-only; migrations are the file names
  embedded in each helper binary. The sha256 is the build's own `.sha256` file,
  re-checked against the DMG on 2026-09-25. "DMG built" is the DMG's file date
  on the build machine. Every one of these builds points at
  `https://dev-api.getvelvt.com` and has the updater off.
- **1.0.0 and 1.0.1**: the DMGs are not on the build machine any more. The
  sha256 is the checksum printed in the GitHub release notes, which matches
  the digest GitHub records for the uploaded asset. The build number of 1.0.0 is from its
  release notes and that of 1.0.1 from the maintainers' release log. Protocol
  and migrations are read from the tag's tree, not from the binary.
- **Notarization submission IDs**: the notarize scripts write each result to the
  same two files in `dist/`, so only the latest build's IDs survive.
  `xcrun notarytool history` under the signing team would list the others; it
  was not consulted for this ledger.
- **Helper version**: every build above, 1.0.11 included, has a helper that
  reports version 1.0.0 (Cargo's fixed version) in `velvt-service --version`,
  in upload batches and at device registration. Builds after 1.0.11 report the
  app's version.

Not in the table:

- **1.1.0 (build 7)**, a notarized DMG produced on 2026-08-17 at protocol 28 to
  exercise the protocol-28 loop against a throwaway database. It was never
  released, and its DMG is no longer in `dist/`. Its build number is below every
  release since 1.0.3, so build ordering still places it before them, but a
  future release numbered 1.1.0 would share its version string.
- **Builds 7, 8 and 11** as release builds: 7 is the artifact above; no DMG or
  record of builds 8 and 11 exists.
- **`v0.1.0`**, a tag in the maintainer's local clone only, at `6bab95f`
  (2026-07-31), which is an ancestor of `develop`. It marks no notarized build.
  It has never been pushed, and it is being left as it is.

## Cutting the next release

The version and build number live in `swift-client/Configs/Version.xcconfig`
and nowhere else: Debug and Release include it, the Makefile reads its defaults
from it, and `rust-service/build.rs` stamps the helper from it.

1. Raise `MARKETING_VERSION` and `CURRENT_PROJECT_VERSION` in
   `Version.xcconfig`, and merge that change.
2. From a clean checkout of the commit to ship, run `make alpha-dmg` (see
   [`shipping-a-testable-dmg.md`](shipping-a-testable-dmg.md)). Before it
   builds anything, `scripts/release_provenance.sh check-release` refuses:
   - uncommitted or untracked changes (there is no override for a
     distributable build);
   - a version or build passed on the command line that differs from
     `Version.xcconfig`;
   - a version already tagged at another commit;
   - an untagged version that is already in this table, or a build number not
     higher than every build in it.

   The app records the commit in `Info.plist` as `VelvtSourceCommit`, the helper
   reports the same commit from `velvt-service --source-commit`, and
   `scripts/verify_release.sh` checks that they match. For a production build
   it also requires a clean 40-character commit. After notarization succeeds,
   the target tags `v<version>` at that commit, locally.
3. Push the tag (`git push origin v<version>`), and add the row to this table
   in the same pull request that records anything else about the release. Copy
   the notarization IDs from `dist/app-notarization-result.plist` and
   `dist/notarization-result.plist` before the next build overwrites them.

`make package-release` and `VELVT_ALLOW_LOCAL_DMG=1 make dmg` refuse a dirty
tree too. For a local experiment that will never leave the build machine,
`VELVT_ALLOW_DIRTY_TREE=1` lets them build, and the result records
`<commit>-dirty`.
