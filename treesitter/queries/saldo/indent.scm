; The contents of flow and schedule blocks are indented one level.
[
  (flow_decl)
  (schedule_decl)
] @indent.begin

; The closing brace falls back to the level of the opening declaration.
"}" @indent.end
