# RAW test fixtures

The RAW tests in `src/raw/tests.rs` decode real camera files so that CI exercises the actual `rawler` decode path, the sensor-layout checks, the Bayer and X-Trans demosaic paths, orientation, colour matrices and as-shot white balance. Synthetic images cannot catch those regressions.

The camera files are **not committed**. `fixtures.txt` pins each one by URL, SHA-256 and size, and `scripts/fetch-raw-fixtures.sh` downloads them into `testdata/raw/cache/` (git-ignored). See [docs/DEVELOPMENT.md](../../docs/DEVELOPMENT.md#raw-test-fixtures) for local usage.

## Sources and licenses

Every file comes from the [raw.pixls.us](https://raw.pixls.us/) sample archive and is released by its contributor under [Creative Commons Zero 1.0 (public domain)](https://creativecommons.org/publicdomain/zero/1.0/). The license of each sample was checked against the archive's repository listing (`https://raw.pixls.us/json/getrepository.php?set=all`), and the SHA-256 recorded there matches the pinned hashes. Only CC0 samples are used; the archive also hosts some non-CC0 imports, which must not be added here.

| Format | Camera | File | Size | Purpose |
| --- | --- | --- | --- | --- |
| NEF | Nikon D70 | `Nikon/D70/20170902_0047.NEF` | 5.2 MB | Nikon Bayer |
| CR2 | Canon EOS Digital Rebel XT | `Canon/EOS Digital Rebel XT/IMG_8728.CR2` | 7.1 MB | Canon CR2, portrait orientation |
| CR3 | Canon EOS R6 Mark III | `Canon/Canon EOS R6 Mark III/IMG_4244.CR3` | 6.7 MB | Canon CR3 (CRAW) |
| CR3 | Canon EOS R5 Mark II (APS-C crop, CRAW) | `Canon/Canon EOS R5m2/APS-C_CRAW.CR3` | 7.2 MB | Canon CR3 crop-mode capture (panicked before rawler 0.8.0) |
| CRW | Canon EOS D30 | `Canon/EOS D30/CRW_2444.CRW` | 2.9 MB | Canon CIFF |
| RAF | Fujifilm X20 | `Fujifilm/X20/DSCF7451.RAF` | 18.6 MB | Fujifilm X-Trans (6x6) |
| ARW | Sony ILCE-7S | `Sony/ILCE-7S/DSC04126.ARW` | 5.9 MB | Sony Bayer; shot in APS-C crop mode, so it decodes to 2768x1848 rather than the full-frame 4240x2832 |
| CR2 | Canon EOS 5D Mark II (sRAW2) | `Canon/EOS 5D Mark II/10.canon.sraw2.cr2` | 10.9 MB | Unsupported layout, must be rejected |

All files are served from `https://raw.pixls.us/data/<Make>/<Model>/<file>`. The sRAW file is the negative test: Canon sRAW/mRAW is intentionally unsupported, and the test only asserts that decoding fails.

## Adding or changing a fixture

1. Pick the smallest CC0 sample for the format and confirm its license in the repository listing above (the license column must be Creative Commons 0).
2. Download it, compute `sha256sum` and the byte size, and add a line to `fixtures.txt` with the expected camera name and oriented dimensions (run the tests to see what the decoder reports and sanity-check them against the camera).
3. Add a test in `src/raw/tests.rs` if the file covers a new format, and list it in the table above.

Changing `fixtures.txt` changes the CI cache key, so the new set is downloaded once and cached.
