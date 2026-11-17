# Verification decks

Every `.cir` file here is simulated by spx and by ngspice (run in Docker as a black-box
reference, see `tools/ngspice`), and the results are compared:

```sh
cargo run -p spx-verify            # all decks
cargo run -p spx-verify -- --cached -v verify/decks/bjt_dc.cir
```

The score of a variable is `max|ours − ref| / (rtol·max|ref| + atol)`; it passes when ≤ 1.
Defaults: `rtol = 2e-3` (OP/DC/AC), `rtol_tran = 1e-2`, `rtol_tran_i = 3e-2` (transient
currents), `rtol_meas = 1e-2`, `atol_v = 1 µV`, `atol_i = 1 nA`. A deck can override them with a
comment line such as `* verify: rtol_tran=3e-2`.

All decks are original circuits written for this project. Raw files go to `verify/out/`
(ignored by git).
