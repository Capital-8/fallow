### Fixed

- **`fallow coverage upload-inventory` now names a function that is assigned to a declared variable.** For `let startLoad; startLoad = () => {}`, the inventory named the function `(anonymous_N)`. `oxc-coverage-instrument` 0.16.0 and later name it after the variable (`startLoad`), so the runtime function and the inventory function did not join in Fallow Cloud. The inventory now follows the same rule as the instrumenter for an assigned function:
  - Only a plain `=` gives a name. A compound or logical operator (`+=`, `||=`, `&&=`, `??=`) gives none.
  - Parentheses around the value do not matter: `x = (() => {})` gives `x`.
  - A function with its own name keeps it: `x = function own() {}` gives `own`.
  - An assigned class does not give the name to its first field arrow.
  - A function inside the target, as in `f(() => 1).x = () => 2`, does not take the target name.

  The stable id of a renamed function changes, because the name is one of its inputs.
