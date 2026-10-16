# Contributing to xdj-az-emu

The emulator runs the XDJ-AZ's own program on your computer so mods can be developed
without a deck. Read [How it works](docs/how-it-works.md) before changing the guest side.

## Before you open a pull request

Open an issue first for anything larger than a fix. Never add firmware, firmware art,
keys or anything copied from a deck to the repository: the system is pulled from the
developer's own deck and stays on their computer.

## Checks to run

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
python3 tools/check-lf.py                  # files that must keep LF line endings
python3 tools/az-emu-test.py               # the full suite; needs a pulled system and a built image
```

`az-emu-test.py` boots the emulator and checks the boot, the mods, the controls and stems,
each stage with its own control. Run it before a change to the guest or the image build.

## Style

- Rust: `rustfmt.toml` and Clippy as configured.
- C in `guest/`: `.clang-format`, comments that explain the code and nothing else.
- Each file keeps its SPDX header; files kept from upstream keep their license (see
  [ATTRIBUTION.md](ATTRIBUTION.md)).

## Hygiene

Every push and pull request runs `tools/public-hygiene`, which keeps personal names, dates, private paths and em dashes out of code, docs and commit messages (rules in `.public-hygiene.json`). Run `tools/install-hooks.sh` once in your clone to get the same check before every push.

By contributing you agree that your contribution is licensed under the license of the
files you change (see the SPDX header of each file and [LICENSE](LICENSE)).
