### Fixed

- **`fallow coverage upload-inventory` now names a function that is assigned to a declared variable.** For `let startLoad; startLoad = () => {}`, the inventory named the function `(anonymous_N)`. `oxc-coverage-instrument` 0.16.0 and later name it after the variable (`startLoad`), so the runtime function and the inventory function did not join in Fallow Cloud. The inventory now follows the same rule as the instrumenter for an assigned function:
  - Only a plain `=` gives a name. A compound or logical operator (`+=`, `||=`, `&&=`, `??=`) gives none.
  - Parentheses around the value do not matter: `x = (() => {})` gives `x`.
  - An assigned class does not give the name to its first field arrow.
  - A function inside the target, as in `f(() => 1).x = () => 2`, does not take the target name.

- **`fallow coverage upload-inventory` now ranks the own name of a function above the name of its binding, as `oxc-coverage-instrument` and `Function.prototype.name` do.** The inventory gave `f` for `const f = function g() {}` and `k` for `{ k: function own() {} }`. The instrumenter gives `g` and `own`, so these functions did not join in Fallow Cloud. The inventory now uses one rule for a declarator, an object property and an assignment:
  - A function with its own name keeps it: `const f = function g() {}` gives `g`, `{ k: function own() {} }` gives `own` and `x = function own() {}` gives `own`.
  - Parentheses around the value do not matter: `const f = (() => {})` gives `f` and `{ k: (() => {}) }` gives `k`.
  - A class value takes no binding name: `const C = class D {}` names only its constructor and methods.

  The stable id of a renamed function changes, because the name is one of its inputs.
