# Manrope

Manrope by Mikhail Sharanda and The Manrope Project Authors, used under the
SIL Open Font License, Version 1.1. The full license is in `OFL.txt` beside
this file, and ships with the fonts inside the app at
`Velvt.app/Contents/Resources/Fonts/`.

Copyright notice, as embedded in each vendored face (name ID 0, version 4.504):

> Copyright 2019 The Manrope Project Authors (https://github.com/sharanda/manrope)

`OFL.txt` is the upstream project's license file, copied unmodified from
<https://github.com/googlefonts/manrope/blob/468c0dbe38efa331b80bfe9448256abe27be44c3/OFL.txt>
(sha256 `58172e0c0fac2cda8a37b348164bb55e44b0e69051e557e92b1d3f6910141f7b`).
Its first line carries upstream's own notice, "Copyright 2018 The Manrope
Project Authors". The `sharanda/manrope` repository named in the faces' notice
no longer resolves; `googlefonts/manrope` is the repository Google Fonts builds
Manrope from.

Three static weights are vendored here because the Velvt brand system
(`Style Guide`, 01/2026) specifies exactly three: EXTRABOLD for display,
BOLD for headings and labels, REGULAR for body. The variable font is not
used — static faces keep the rendered weights identical to the guide on
every macOS version, and keep the bundle small.

The OFL permits bundling and redistributing the fonts inside an application,
provided that no face is sold by itself, and that the copyright notice and the
license travel with every copy (OFL conditions 1 and 2). Keep `OFL.txt` and
this file in this directory: `scripts/verify_release.sh` refuses a release
bundle without `OFL.txt`.
