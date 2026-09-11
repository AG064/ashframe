## ASHFRAME — the heads-up display.
##
## The HUD reads the game node and never writes to it. Every number on screen
## comes from a getter, which is what keeps a layout mistake from becoming a
## gameplay one: the worst this script can do is draw the wrong thing.
##
## It is built in code rather than in a scene file because almost all of it is
## the same four shapes repeated — a panel, a label, a bar, a bracket — and a
## scene of four hundred nodes is harder to read than the loop that makes them.

extends Control

const ACCENT := Color(0.88, 0.64, 0.28)
const CYAN := Color(0.44, 0.83, 0.92)
const DANGER := Color(0.88, 0.34, 0.24)
const PANEL := Color(0.05, 0.06, 0.08, 0.62)
const EDGE := Color(0.88, 0.64, 0.28, 0.35)
const DIM := Color(0.72, 0.75, 0.79)

var game: Node

var _reticle: Control
var _reticle_dot: ColorRect
var _brackets: Array[ColorRect] = []
var _objective: Label
var _phase: Label
var _hint: Label

var _health_fill: ColorRect
var _health_label: Label
var _stability_fill: ColorRect
var _energy_fill: ColorRect

var _ammo: Label
var _reload: ColorRect
var _systems: Label

var _timer: Label
var _enemies: Label
var _boss: Control
var _boss_fill: ColorRect

var _overlay: Control
var _overlay_title: Label
var _overlay_body: Label
var _overlay_foot: Label

var _vignette: ColorRect
var _shake_flash: float = 0.0
var _last_health: float = 0.0
var _last_phase: String = ""


func _ready() -> void:
	game = get_node_or_null("../Game")
	set_anchors_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_IGNORE

	_build_vignette()
	_build_reticle()
	_build_status()
	_build_readouts()
	_build_boss()
	_build_overlay()

	if game != null:
		_last_health = game.health()


## A red wash at the edges when the frame takes a hit. The only effect here that
## is not a readout: a damage number in the corner is easy to miss while the
## middle of the screen is where the player is actually looking.
func _build_vignette() -> void:
	_vignette = ColorRect.new()
	_vignette.set_anchors_preset(Control.PRESET_FULL_RECT)
	_vignette.color = Color(0.7, 0.06, 0.04, 0.0)
	_vignette.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_vignette)


func _build_reticle() -> void:
	_reticle = Control.new()
	_reticle.set_anchors_preset(Control.PRESET_FULL_RECT)
	_reticle.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_reticle)

	_reticle_dot = ColorRect.new()
	_reticle_dot.custom_minimum_size = Vector2(3, 3)
	_reticle_dot.size = Vector2(3, 3)
	_reticle_dot.color = Color(0.95, 0.95, 0.95, 0.85)
	_reticle.add_child(_reticle_dot)

	# Eight ticks rather than four: the diagonals are what make the reticle read
	# as a gunsight instead of as a cross.
	for i in 8:
		var tick := ColorRect.new()
		tick.custom_minimum_size = Vector2(2, 10)
		tick.size = Vector2(2, 10)
		tick.color = Color(0.95, 0.95, 0.95, 0.5)
		_reticle.add_child(tick)

	for i in 4:
		var corner := ColorRect.new()
		corner.custom_minimum_size = Vector2(22, 2)
		corner.size = Vector2(22, 2)
		corner.color = CYAN
		corner.visible = false
		_reticle.add_child(corner)
		_brackets.append(corner)


func _panel(parent: Control, anchor: int, offset: Vector2, size: Vector2) -> Control:
	var box := PanelContainer.new()
	box.set_anchors_preset(anchor)
	box.position = offset
	box.custom_minimum_size = size
	box.size = size
	box.mouse_filter = Control.MOUSE_FILTER_IGNORE

	var style := StyleBoxFlat.new()
	style.bg_color = PANEL
	style.border_color = EDGE
	style.set_border_width_all(1)
	style.set_corner_radius_all(2)
	style.content_margin_left = 14
	style.content_margin_right = 14
	style.content_margin_top = 8
	style.content_margin_bottom = 8
	box.add_theme_stylebox_override("panel", style)
	parent.add_child(box)
	return box


func _label(parent: Control, text: String, size: int, colour: Color) -> Label:
	var label := Label.new()
	label.text = text
	label.add_theme_font_size_override("font_size", size)
	label.add_theme_color_override("font_color", colour)
	label.add_theme_color_override("font_shadow_color", Color(0, 0, 0, 0.7))
	label.add_theme_constant_override("shadow_offset_x", 1)
	label.add_theme_constant_override("shadow_offset_y", 1)
	parent.add_child(label)
	return label


func _bar(parent: Control, width: float, height: float, colour: Color, label: String) -> Array:
	var row := HBoxContainer.new()
	row.add_theme_constant_override("separation", 8)
	row.mouse_filter = Control.MOUSE_FILTER_IGNORE
	parent.add_child(row)

	var tag := _label(row, label, 12, DIM)
	tag.custom_minimum_size = Vector2(74, 0)
	tag.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	var track := ColorRect.new()
	track.color = Color(0.10, 0.11, 0.13, 0.85)
	track.custom_minimum_size = Vector2(width, height)
	track.size = Vector2(width, height)
	track.mouse_filter = Control.MOUSE_FILTER_IGNORE
	row.add_child(track)

	var fill := ColorRect.new()
	fill.color = colour
	fill.position = Vector2.ZERO
	fill.size = Vector2(width, height)
	fill.mouse_filter = Control.MOUSE_FILTER_IGNORE
	track.add_child(fill)

	var value := _label(row, "", 12, Color.WHITE)
	value.custom_minimum_size = Vector2(58, 0)
	value.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	value.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	return [track, fill, value]


func _build_status() -> void:
	var box := _panel(self, Control.PRESET_BOTTOM_LEFT, Vector2(28, -190), Vector2(360, 158))
	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation", 8)
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	box.add_child(column)

	var title := _label(column, "ASHFRAME // INTEGRITY", 12, ACCENT)
	title.add_theme_constant_override("outline_size", 0)

	var health := _bar(column, 208, 16, Color(0.78, 0.26, 0.22), "HULL")
	_health_fill = health[1]
	_health_label = health[2]

	var stability := _bar(column, 208, 8, Color(0.85, 0.72, 0.36), "BALANCE")
	_stability_fill = stability[1]

	var energy := _bar(column, 208, 8, CYAN, "ENERGY")
	_energy_fill = energy[1]

	_phase = _label(column, "", 12, DIM)


func _build_readouts() -> void:
	var box := _panel(self, Control.PRESET_BOTTOM_RIGHT, Vector2(-300, -190), Vector2(272, 158))
	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation", 6)
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	box.add_child(column)

	var title := _label(column, "VK-40 AUTOCANNON", 12, ACCENT)

	_ammo = _label(column, "42 / 42", 30, Color.WHITE)

	var track := ColorRect.new()
	track.color = Color(0.10, 0.11, 0.13, 0.85)
	track.custom_minimum_size = Vector2(200, 4)
	track.mouse_filter = Control.MOUSE_FILTER_IGNORE
	column.add_child(track)
	_reload = ColorRect.new()
	_reload.color = ACCENT
	_reload.size = Vector2(0, 4)
	track.add_child(_reload)

	_systems = _label(column, "", 12, DIM)

	var top := _panel(self, Control.PRESET_TOP_RIGHT, Vector2(-300, 28), Vector2(272, 78))
	var tcol := VBoxContainer.new()
	tcol.add_theme_constant_override("separation", 4)
	tcol.mouse_filter = Control.MOUSE_FILTER_IGNORE
	top.add_child(tcol)
	_timer = _label(tcol, "00:00", 22, Color.WHITE)
	_enemies = _label(tcol, "", 12, DIM)

	var objective_box := _panel(self, Control.PRESET_TOP_LEFT, Vector2(28, 28), Vector2(420, 66))
	var ocol := VBoxContainer.new()
	ocol.add_theme_constant_override("separation", 4)
	ocol.mouse_filter = Control.MOUSE_FILTER_IGNORE
	objective_box.add_child(ocol)
	_objective = _label(ocol, "", 17, Color.WHITE)
	_hint = _label(ocol, "", 12, CYAN)


func _build_boss() -> void:
	_boss = _panel(self, Control.PRESET_CENTER_TOP, Vector2(-260, 22), Vector2(520, 54))
	var column := VBoxContainer.new()
	column.add_theme_constant_override("separation", 6)
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_boss.add_child(column)
	_label(column, "ASHFRAME PATRIARCH", 15, DANGER)
	var track := ColorRect.new()
	track.color = Color(0.10, 0.11, 0.13, 0.9)
	track.custom_minimum_size = Vector2(480, 12)
	track.mouse_filter = Control.MOUSE_FILTER_IGNORE
	column.add_child(track)
	_boss_fill = ColorRect.new()
	_boss_fill.color = DANGER
	_boss_fill.size = Vector2(480, 12)
	track.add_child(_boss_fill)
	_boss.visible = false


func _build_overlay() -> void:
	_overlay = Control.new()
	_overlay.set_anchors_preset(Control.PRESET_FULL_RECT)
	_overlay.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(_overlay)

	var wash := ColorRect.new()
	wash.set_anchors_preset(Control.PRESET_FULL_RECT)
	wash.color = Color(0.02, 0.03, 0.04, 0.55)
	wash.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_overlay.add_child(wash)

	var column := VBoxContainer.new()
	column.set_anchors_preset(Control.PRESET_CENTER)
	column.alignment = BoxContainer.ALIGNMENT_CENTER
	column.add_theme_constant_override("separation", 18)
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_overlay.add_child(column)

	_overlay_title = _label(column, "OPERATION ASHEN YARD", 54, Color.WHITE)
	_overlay_title.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER

	_overlay_body = _label(column, "", 16, DIM)
	_overlay_body.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER

	_overlay_foot = _label(column, "", 18, ACCENT)
	_overlay_foot.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER


func _process(delta: float) -> void:
	if game == null:
		game = get_node_or_null("../Game")
		if game == null:
			return

	_update_reticle()
	_update_status()
	_update_readouts()
	_update_boss()
	_update_overlay()
	_update_vignette(delta)


func _update_reticle() -> void:
	var centre := size * 0.5
	var speed: float = game.speed()
	# The reticle opens as the mech speeds up, which is the only feedback the
	# player gets about how fast they are actually going.
	var spread := 12.0 + clampf(speed / 48.0, 0.0, 1.4) * 26.0

	_reticle_dot.position = centre - Vector2(1.5, 1.5)
	var ticks := _reticle.get_children().slice(1, 9)
	for i in ticks.size():
		var tick: ColorRect = ticks[i]
		var angle := TAU * float(i) / 8.0
		var direction := Vector2(sin(angle), cos(angle))
		tick.position = centre + direction * spread - tick.size * 0.5
		tick.rotation = -angle

	_update_brackets(centre)


func _update_brackets(centre: Vector2) -> void:
	for corner in _brackets:
		corner.visible = false
	if not game.missiles_locked():
		return

	var camera := get_viewport().get_camera_3d()
	if camera == null:
		return
	var world: Vector3 = game.lock_world_position()
	if world == Vector3.ZERO:
		return
	var point := camera.unproject_position(world)
	if camera.is_position_behind(world):
		return

	var radius := clampf(1400.0 / maxf(game.lock_distance(), 8.0), 22.0, 130.0)
	var offsets := [
		Vector2(-radius, -radius), Vector2(radius, -radius),
		Vector2(-radius, radius), Vector2(radius, radius),
	]
	for i in _brackets.size():
		var corner: ColorRect = _brackets[i]
		corner.visible = true
		var offset: Vector2 = offsets[i]
		# A ColorRect has no flip flag, so the inward-pointing tick on the right
		# of the target is the same rectangle turned about its own end.
		corner.pivot_offset = Vector2(11, 1)
		corner.rotation = 0.0 if offset.x < 0.0 else PI
		corner.position = point + offset - Vector2(11, 1)


func _update_status() -> void:
	var health: float = game.health()
	var health_max: float = maxf(game.health_max(), 1.0)
	var fraction: float = clampf(health / health_max, 0.0, 1.0)
	_health_fill.size.x = _health_fill.get_parent().size.x * fraction
	# Amber past a third, red past a sixth. Two thresholds, because one is a
	# warning and the other is a decision about whether to repair.
	if fraction < 0.16:
		_health_fill.color = DANGER
	elif fraction < 0.36:
		_health_fill.color = ACCENT
	else:
		_health_fill.color = Color(0.78, 0.26, 0.22)
	_health_label.text = "%d" % roundi(health)

	var stability: float = clampf(game.stability() / maxf(game.stability_max(), 1.0), 0.0, 1.0)
	_stability_fill.size.x = _stability_fill.get_parent().size.x * stability
	_stability_fill.color = DANGER if stability > 0.75 else Color(0.85, 0.72, 0.36)

	var energy: float = clampf(game.energy() / maxf(game.energy_max(), 1.0), 0.0, 1.0)
	_energy_fill.size.x = _energy_fill.get_parent().size.x * energy

	var phase: String = game.mission_phase()
	_phase.text = "%s   //   %d HOSTILE%s" % [
		phase.to_upper(), game.enemies_left(), "" if game.enemies_left() == 1 else "S"
	]
	if game.staggered():
		_phase.text = "!!  FRAME STAGGERED  !!"
		_phase.add_theme_color_override("font_color", DANGER)
	else:
		_phase.add_theme_color_override("font_color", DIM)


func _update_readouts() -> void:
	_ammo.text = "%d / %d" % [game.ammo(), game.magazine()]
	if game.reloading():
		_ammo.add_theme_color_override("font_color", ACCENT)
		_reload.size.x = _reload.get_parent().size.x * game.reload_progress()
	else:
		_ammo.add_theme_color_override("font_color", Color.WHITE)
		_reload.size.x = 0.0

	var systems := []
	if game.blade_ready():
		systems.append("[V] BLADE")
	if game.missiles_ready():
		systems.append("[RMB] VOLLEY")
	if game.repairs() > 0:
		systems.append("[F] REPAIR x%d" % game.repairs())
	if game.repairing():
		systems.append("REPAIRING...")
	if game.assault_boosting():
		systems.append("ASSAULT BOOST")
	_systems.text = "   ".join(systems)

	_timer.text = _clock(game.elapsed())
	var distance: float = game.lock_distance()
	if game.missiles_locked() and distance > 0.0:
		_enemies.text = "LOCK  %s  %dm" % [game.lock_name(), roundi(distance)]
	else:
		_enemies.text = "%d KILLS   %d PTS" % [game.kills(), game.score()]

	_objective.text = game.objective()
	_hint.text = game.hint()


func _update_boss() -> void:
	var present: bool = game.boss_present()
	_boss.visible = present and game.phase_label() == "playing"
	if not present:
		return
	var fraction: float = clampf(game.boss_health() / maxf(game.boss_health_max(), 1.0), 0.0, 1.0)
	_boss_fill.size.x = _boss_fill.get_parent().size.x * fraction


func _update_overlay() -> void:
	var phase: String = game.phase_label()
	if phase == _last_phase:
		return
	_last_phase = phase

	match phase:
		"ready":
			_overlay.visible = true
			_overlay_title.text = "OPERATION ASHEN YARD"
			_overlay_title.add_theme_color_override("font_color", Color.WHITE)
			_overlay_body.text = (
				"ASHEN YARD, 04:40. A skirmisher screen is holding the yard and a siege\n"
				+ "pair is dug in behind it. Break the screen, survive the artillery, and\n"
				+ "bring down the Patriarch.\n\n" + game.control_help()
			)
			_overlay_foot.text = "PRESS ENTER TO DEPLOY"
		"paused":
			_overlay.visible = true
			_overlay_title.text = "PAUSED"
			_overlay_title.add_theme_color_override("font_color", Color.WHITE)
			_overlay_body.text = "ESC resumes.  R restarts the operation."
			_overlay_foot.text = ""
		"results":
			_overlay.visible = true
			if game.won():
				_overlay_title.text = "MISSION COMPLETE"
				_overlay_title.add_theme_color_override("font_color", ACCENT)
			else:
				_overlay_title.text = "FRAME LOST"
				_overlay_title.add_theme_color_override("font_color", DANGER)
			_overlay_body.text = (
				"TIME       %s\nKILLS      %d\nSCORE      %d\nDAMAGE     %d\nROUNDS     %d\nREPAIRS    %d"
				% [
					_clock(game.elapsed()), game.kills(), game.score(),
					roundi(game.damage_taken()), game.shots_fired(), game.repairs_used(),
				]
			)
			_overlay_foot.text = "PRESS ENTER FOR A NEW OPERATION"
		_:
			_overlay.visible = false


func _update_vignette(delta: float) -> void:
	var health: float = game.health()
	if health < _last_health - 0.5:
		_shake_flash = 1.0
	_last_health = health
	_shake_flash = maxf(0.0, _shake_flash - delta * 2.4)
	# Deliberately faint. At four tenths of full opacity this is a red screen
	# rather than a warning, and a player under sustained fire spends the whole
	# fight looking through it.
	_vignette.color = Color(0.7, 0.06, 0.04, _shake_flash * _shake_flash * 0.22)


func _clock(seconds: float) -> String:
	var total := int(seconds)
	return "%02d:%02d" % [total / 60, total % 60]
