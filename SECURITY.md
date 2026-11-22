# Security policy

Only the latest released version of Sansho receives security fixes.

## Private reporting

Use **Report a vulnerability** on this repository's GitHub Security page to contact the maintainers privately. Include the release or commit, operating system, enabled features, impact and a minimal example with any confidential circuit data removed. Keep vulnerability details out of public issues and discussions. [Report a vulnerability privately](https://github.com/hayatree/sansho/security/advisories/new)

## Scope

- Report failures when parsing or elaborating untrusted netlists, include files and models, including unexpected file access or resource exhaustion.
- The stdio MCP server runs with the host process's file permissions. Reports about tool input, server-side path handling, session data and resource consumption are in scope.
- The optional Verilog-A JIT compiles user-supplied Verilog-A to native machine code that executes in the simulator process. Compiler, runtime and native cache issues are in scope. Set `SANSHO_VA_JIT=0` to select the interpreter in a JIT-enabled build.
- Large circuits, sweeps, transient runs and compiled models can consume substantial CPU time, memory and disk space. When processing untrusted inputs, isolate the process, restrict readable and writable paths and impose external CPU, memory, output and wall-time limits. The MCP Monte Carlo tool accepts up to 256 runs and limits the serialized report to 256 KiB. Other tools and the CLI have different resource requirements. [Monte Carlo limits](docs/monte-carlo.md#cli-and-api)
