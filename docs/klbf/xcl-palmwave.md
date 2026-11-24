# XCL and Palmwave

Upstream: https://github.com/ivall/xcl (GPL-2.0) and its fork https://github.com/fablinoo/palmwave (GPL-2.0). We test them unmodified.

## Build

```sh
git clone https://github.com/ivall/xcl        # or fablinoo/palmwave
cargo build --release --workspace
```

## Run

Always offline: one of our early runs tried to upload a 27 MB crash dump. [`run-test.ps1`](run-test.ps1) sets `XCL_NETWORK=0` for you.

```powershell
$env:XCL_ADAPTER = 'radeon'
$env:XCL_PRESS_A_AT = '25,35,45,60'
# Palmwave only: the six XCL_PATCH entries and XCL_DSTORAGE_BCPACK_FILL=neutral from its README
powershell -File <klbf-lab>\docs\klbf\run-test.ps1 -Project xcl -Exe target\release\xcl.exe -Game <your dump folder> -Minutes 3
```

Palmwave shader precompile (once): `xcl shaders <dump>\shaders.rpf --precompile --only cs --pipelines`.
