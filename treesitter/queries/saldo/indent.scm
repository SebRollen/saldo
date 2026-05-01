; Entry and param schedule blocks indent their contents one level.
[
  (entry_decl)
  (param_decl)
] @indent.begin

; The closing brace falls back to the level of the opening declaration.
"}" @indent.end
