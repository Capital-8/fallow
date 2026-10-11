# fallow-regress

An ECMAScript regular expression engine maintained by Fallow, derived from
[regress](https://github.com/ridiculousfish/regress). The library target remains
`regress` so existing imports and examples continue to work.

This package adds shared compilation and matching budgets. Compilation can
refuse work, cumulative allocation requests or tree depth with a typed limit;
matching can return budget exhaustion separately from no match. A caller can
reuse the same budget across ordered patterns and requests. Exhaustion remains
sticky. UTF-16 and UCS-2 search entry points are available with `utf16`.

These allowances measure admitted engine operations and storage requests. They
are not a wall-clock deadline, a bound on total process memory, or a guarantee
of complete JavaScript compatibility. Existing unlimited APIs remain available.
Callers processing untrusted patterns should select the bounded APIs and keep
exhaustion distinct from syntax errors and ordinary non-matches.

```sh
cargo add fallow-regress --rename regress --no-default-features --features std,utf16
```

```rust
use regress::{CompileBudget, MatchBudget, Regex};

let mut compilation = CompileBudget::new(100_000, 100_000, 128);
let regex = Regex::from_unicode_with_limits(
    r"^source/(.*)".chars().map(u32::from),
    "",
    &mut compilation,
)?;
let input: Vec<u16> = "source/module".encode_utf16().collect();
let mut matching = MatchBudget::new(10_000);
let result = regex.find_from_ucs2_with_budget(&input, 0, &mut matching)?;
assert_eq!(result.unwrap().range(), 0..13);
# Ok::<(), Box<dyn std::error::Error>>(())
```

The example allowances illustrate the API and are not recommended limits for
an application. Default features retain the upstream `std` and Pike VM
configuration. `pattern` requires nightly Rust. The selected bounded Unicode
configuration is `--no-default-features --features std,utf16`.

See [provenance and maintenance](PROVENANCE.md), the retained
[upstream documentation](UPSTREAM.md), and the API documentation for details.
The original author is ridiculousfish. This fork retains the upstream
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE) license choice.
