# Provenance and maintenance

This package derives from the crates.io `regress` 0.12.0 source published by
ridiculousfish, corresponding to upstream Git commit
`aae35dee1596cc9a9caaad2425b088081c09e5d5`. The original repository is
<https://github.com/ridiculousfish/regress>. Both upstream license files and
source attribution are retained unchanged. `UPSTREAM.md` preserves that
release's README; it describes the upstream package, not every fork addition.

Fallow maintains this source in `crates/regex`, under the distinct Cargo package
name `fallow-regress`. Its library name remains `regress`. Its version follows
the Fallow workspace release version. It does not replace transitive
requirements for the upstream `regress` package.

The fork adds shared fallible compilation and Unicode-search budgets, charged
storage helpers and regression coverage for refusal propagation and retained
matching behavior. Compilation accounting includes parser, set operations,
optimizer, start predicate and bytecode generation. Matching accounting includes
search scans and backtracking. Separate parser fixes preserve duplicate named
backreference behavior, Unicode-set operands and empty alternatives, syntactic
string-set negation restrictions and nested ignore-case complements.
Recursive parser helpers keep supported nesting within the default test-thread
stack. Subprocess tests check parsing and bounded matching without the workspace
stack override; deep unlimited lookaround execution retains upstream limits.

Budget tests exercise typed refusal and sticky reuse. Oracle tests preserve
expected captures and UTF-16 ranges, including distinctions established from
ECMAScript algorithms where JavaScript engines disagree. They do not claim
universal conformance or a fixed total-memory bound.

To reconstruct the source delta, download and extract the official crates.io
`regress` 0.12.0 archive, then compare its `src` and `tests` directories with this
package. Cargo metadata is maintained here: the inherited workspace version,
repository and toolchain metadata, package identity, package contents and test
targets are intentional packaging differences. The package preserves upstream
source lint attributes; it does not inherit Fallow's additional first-party
lint policy. Canonical workspace Clippy still runs on the package with warnings
as errors and the repository Clippy configuration. Item-specific annotations
retain upstream control flow, executor calling conventions and unlimited/test
map choices where repository-only thresholds differ. They do not suppress
checks for the rest of the crate.

For an upstream update:

1. Record the upstream release and source revision and review the complete
   source delta, including licenses and generated Unicode tables.
2. Preserve shared-budget accounting and first-refusal behavior across every
   changed compiler or executor path.
3. Run the complete default and `std,utf16` suites, alloc configurations, strict
   workspace checks and public-input semantic/resource comparisons.
4. Verify the Cargo archive, extracted standalone suites and a package-derived
   consumer. Recheck product behavior and performance when connecting callers.

Upstream convergence is preferred when an upstream release supports the required
shared APIs and passes these checks. No automatic synchronization is promised.
