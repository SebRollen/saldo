; Blocks indent their contents one level.
[
  (arms)
  (block)
  (balances_decl)
  (module_body)
  (mod_decl)
  (fn_decl)
] @indent.begin

; The closing brace falls back to the level of the opening declaration.
"}" @indent.end
