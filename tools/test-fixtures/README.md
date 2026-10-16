# Test fixtures

Deliberately broken libraries that prove `tools/az-emu-test.py` can FAIL:

- `crashmod.c`: an `azmod.so` whose constructor aborts inside EP147 only.
- `nostems.c`: an `azstem.so` that loads but has no stems engine.

Build them with any aarch64 GCC, into the ignored `build/` folder. With Docker, a
stock Ubuntu image is enough (the mod toolchain image `azmod-build:18.04`, defined in
the AZ-MODS source's `docker/azmod-build.Dockerfile`, works too):

    docker run --rm --platform=linux/arm64 -v "$PWD":/r -w /r ubuntu:18.04 sh -c \
      "apt-get update -qq && apt-get install -y -qq gcc libc6-dev >/dev/null && \
       mkdir -p build/test-fixtures && cd tools/test-fixtures && \
       gcc -O2 -fPIC -shared -o ../../build/test-fixtures/crashmod.so crashmod.c && \
       gcc -O2 -fPIC -shared -o ../../build/test-fixtures/nostems.so nostems.c"

(In Git Bash on Windows, prefix `MSYS_NO_PATHCONV=1` and use `"$(pwd -W)"`.)

Then, with the published set installed (`tools/az-mods-fetch.py`):

- `python tools/az-emu-test.py --azmod build/test-fixtures/crashmod.so` must FAIL
  mods (and the stages after it).
- `python tools/az-emu-test.py --azstem build/test-fixtures/nostems.so` must FAIL
  stems only.

The harness saves the installed pair before pushing a fixture and puts it back when
the run ends.
