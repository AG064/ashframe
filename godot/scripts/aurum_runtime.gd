; The handle a scene uses to reach the engine.
;
; The autoload exists so scripts never look the node up by path:
; moving it in the scene tree cannot break them.
extends AurumNode
