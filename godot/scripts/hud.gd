## ASHFRAME — the heads-up display.
##
## The HUD reads the game node and never writes to it. Every number on screen
## comes from a `#[func]` getter, which is what keeps a layout mistake from
## becoming a gameplay one: the worst this script can do is draw the wrong thing.
##
## It is built in code rather than in a scene file because almost all of it is
## the same four shapes repeated — a frame, a meter, a label, a rule — and a
## scene of four hundred nodes is harder to read than the loop that makes them.
## The design language is in `hud_theme.gd` and the full-screen cards are in
## `hud_screens.gd`; no colour, size or gap is invented here.
##
## Three rules the rest of the file follows, because this runs every frame: text
## is rewritten only when its value changes, nothing is created after `_ready()`,
## and anything that is not per-frame information — the render rigs behind the
## contact readout, the accessibility preferences — is sampled on a timer.

extends Control

const PANEL_VITALS := Vector2(474.0, 168.0)
const PANEL_WEAPONS := Vector2(438.0, 172.0)
const PANEL_MISSION := Vector2(566.0, 98.0)
const PANEL_CLOCK := Vector2(306.0, 98.0)
const PANEL_BOSS := Vector2(660.0, 66.0)

## How often the world outside the simulation is sampled: the rigs the contact
## readout points at, and the accessibility preferences.
const SAMPLE_PERIOD := 0.25

const MISSILE := "HR-6 MISSILE POD"
const BLADE := "KS-9 ARC BLADE"
const BOSS_NAME := "ASHFRAME PATRIARCH"
## The fractions where the Patriarch changes behaviour. Marked on its bar, so the
## escalation is something the player can see coming rather than something that
## happens to them.
const BOSS_PHASE_2 := 0.6
const BOSS_PHASE_3 := 0.28

var game: Node

var _phase: String = ""

# the live readout
var _reticle: HudReticle
var _mission: HudFrame
var _clock_frame: HudFrame
var _vitals: HudFrame
var _weapons: HudFrame
var _boss: HudFrame
var _hull: HudMeter
var _stability: HudMeter
var _energy: HudMeter
var _hull_value: Label
var _stability_value: Label
var _energy_value: Label
var _vitals_state: Label
var _mission_line: Label
var _objective: Label
var _hint: Label
var _clock: Label
var _tally: Label
var _ammo: Label
var _magazine: Label
var _weapon_state: Label
var _reload: HudMeter
var _reload_caption: Label
var _missile_state: Label
var _blade_state: Label
var _repair_state: Label
var _boss_meter: HudMeter
var _boss_value: Label
var _boss_phase: Label
var _stagger: Label
var _vignette: TextureRect
var _damage_text: Label
var _score_text: Label

var _screens: HudScreens
var _live: Array[Control] = []

# sampled state
var _motion_off: bool = false
var _sample: float = 0.0
var _contacts := PackedVector3Array()
var _contacts_trusted: bool = false
var _player_rig: Node3D
var _entrance: Tween
var _damage_tween: Tween
var _score_tween: Tween
var _hint_tween: Tween
var _objective_tween: Tween

# last written values, so nothing is formatted twice for the same frame
var _last_health: float = 0.0
var _last_health_int: int = -1
var _last_stability_int: int = -1
var _last_energy_int: int = -1
var _last_ammo: int = -1
var _last_kills: int = -1
var _last_score: int = -1
var _last_enemies: int = -1
var _last_mission_phase: String = ""
var _last_seconds: int = -1
var _last_magazine: int = -1
var _last_objective: String = ""
var _last_hint: String = ""
var _last_vitals_state: String = ""
var _last_weapon_state: String = ""
var _last_boss_phase: String = ""
var _last_staggered: bool = false
var _last_boss: bool = false
var _critical: float = 0.0
var _damage_flash: float = 0.0
var _vignette_level: float = -1.0


func _ready() -> void:
	game = get_node_or_null("../Game")
	set_anchors_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	_motion_off = _prefers_less_motion()

	_build_vignette()
	_build_reticle()
	_build_mission()
	_build_clock()
	_build_vitals()
	_build_weapons()
	_build_boss()
	_build_floats()
	_screens = HudScreens.new()
	_screens.set_reduced_motion(_motion_off)
	add_child(_screens)

	_live = [_mission, _clock_frame, _vitals, _weapons]
	_set_live_visible(false)
	if game != null:
		_last_health = game.health()


func _process(delta: float) -> void:
	if game == null:
		game = get_node_or_null("../Game")
		if game == null:
			return

	_sample -= delta
	if _sample <= 0.0:
		_sample = SAMPLE_PERIOD
		_sample_world()

	var phase: String = game.phase_label()
	if phase != _phase:
		_enter_phase(phase)

	# The screens are written once, when the phase changes. Only the live readout
	# and the damage wash are per-frame.
	if phase == "playing":
		_update_live()
	_update_damage(delta)


# ── construction ────────────────────────────────────────────────────────────

## A red wash at the edges when the frame takes a hit, and a slower pulse under
## it when the hull is nearly gone. The only effect here that is not a readout: a
## number in the corner is easy to miss while the middle of the screen is where
## the player is looking.
##
## Deliberately edge-weighted rather than a tint over the whole picture. A wash
## that reaches the middle is a red screen, and a player under sustained fire
## spends the whole fight looking through it.
func _build_vignette() -> void:
	var gradient := Gradient.new()
	gradient.offsets = PackedFloat32Array([0.0, 0.58, 0.84, 1.0])
	gradient.colors = PackedColorArray([
		Color(0.85, 0.10, 0.05, 0.0),
		Color(0.85, 0.10, 0.05, 0.0),
		Color(0.88, 0.12, 0.05, 0.28),
		Color(0.95, 0.16, 0.07, 0.95),
	])
	var texture := GradientTexture2D.new()
	texture.gradient = gradient
	texture.fill = GradientTexture2D.FILL_RADIAL
	texture.fill_from = Vector2(0.5, 0.5)
	texture.fill_to = Vector2(0.5, 0.02)
	texture.width = 256
	texture.height = 256

	_vignette = TextureRect.new()
	_vignette.texture = texture
	_vignette.set_anchors_preset(Control.PRESET_FULL_RECT)
	_vignette.stretch_mode = TextureRect.STRETCH_SCALE
	_vignette.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_vignette.modulate = Color(1.0, 1.0, 1.0, 0.0)
	add_child(_vignette)


func _build_reticle() -> void:
	_reticle = HudReticle.new()
	_reticle.reduced_motion = _motion_off
	add_child(_reticle)


func _build_mission() -> void:
	_mission = HudFrame.new()
	_mission.accent = HudTheme.AMBER
	add_child(_mission)
	_mission.place(
		Control.PRESET_TOP_LEFT,
		Vector2(HudTheme.MARGIN, HudTheme.MARGIN),
		PANEL_MISSION
	)
	_mission.set_meta(&"hud_rest", Vector2(HudTheme.MARGIN, HudTheme.MARGIN))
	_mission.header("OPERATION ASHEN YARD", "")
	_mission.column.add_theme_constant_override("separation", 5)

	_mission_line = HudTheme.label(
		_mission.column, "", HudTheme.MICRO, HudTheme.FAINT, 0.35, 2
	)
	_objective = HudTheme.label(_mission.column, "", HudTheme.BODY, HudTheme.WHITE, 0.3, 0)
	_objective.clip_text = true
	_objective.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	_objective.custom_minimum_size = Vector2(PANEL_MISSION.x - 30.0, 22.0)
	_hint = HudTheme.label(_mission.column, "", HudTheme.LABEL, HudTheme.CYAN, 0.25, 1)
	_hint.clip_text = true
	_hint.text_overrun_behavior = TextServer.OVERRUN_TRIM_ELLIPSIS
	# The hint row is always there, even when it says nothing, so the objective
	# line above it never moves.
	_hint.custom_minimum_size = Vector2(0.0, 16.0)
	_mission.fit()


func _build_clock() -> void:
	_clock_frame = HudFrame.new()
	_clock_frame.accent = HudTheme.AMBER
	_clock_frame.rule_length = 0.0
	add_child(_clock_frame)
	_clock_frame.place(
		Control.PRESET_TOP_RIGHT,
		Vector2(-HudTheme.MARGIN - PANEL_CLOCK.x, HudTheme.MARGIN),
		PANEL_CLOCK
	)
	_clock_frame.set_meta(
		&"hud_rest", Vector2(-HudTheme.MARGIN - PANEL_CLOCK.x, HudTheme.MARGIN)
	)
	_clock_frame.header("", "MISSION TIME")
	_clock_frame.column.add_theme_constant_override("separation", 0)

	_clock = HudTheme.label(_clock_frame.column, "00:00", HudTheme.BIG, HudTheme.WHITE, 0.3, 1)
	_clock.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	_tally = HudTheme.label(_clock_frame.column, "", HudTheme.MICRO, HudTheme.DIM, 0.3, 1)
	_tally.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT


func _build_vitals() -> void:
	_vitals = HudFrame.new()
	_vitals.accent = HudTheme.AMBER
	add_child(_vitals)
	_vitals.place(
		Control.PRESET_BOTTOM_LEFT,
		Vector2(HudTheme.MARGIN, -HudTheme.MARGIN - PANEL_VITALS.y),
		PANEL_VITALS
	)
	_vitals.set_meta(
		&"hud_rest", Vector2(HudTheme.MARGIN, -HudTheme.MARGIN - PANEL_VITALS.y)
	)
	_vitals.header("FRAME INTEGRITY", "ASHFRAME-01")
	_vitals.column.add_theme_constant_override("separation", 6)

	var hull := _meter_row(_vitals.column, "HULL", HudTheme.RED, 17.0, HudTheme.BIG)
	_hull = hull[0]
	_hull_value = hull[1]
	_hull.danger_below = 0.16
	_hull.warn_below = 0.36
	_hull.segments = 10

	var stability := _meter_row(_vitals.column, "STAB", HudTheme.AMBER, 8.0, HudTheme.LABEL)
	_stability = stability[0]
	_stability_value = stability[1]
	# Stability is a stagger gauge: it fills up as the frame is hit, so a full
	# bar is the warning, and this is the one meter that reads upwards.
	_stability.danger_above = 0.75
	_stability.accent = HudTheme.AMBER

	var energy := _meter_row(_vitals.column, "ENRGY", HudTheme.CYAN, 8.0, HudTheme.LABEL)
	_energy = energy[0]
	_energy_value = energy[1]
	# The floor the frame needs before it will boost, marked on the rail.
	_energy.notches = PackedFloat32Array([0.08])

	_vitals_state = HudTheme.label(_vitals.column, "", HudTheme.MICRO, HudTheme.DIM, 0.35, 2)
	_vitals_state.custom_minimum_size = Vector2(0.0, 14.0)
	_vitals.fit()


## One row of the vitals cluster: a caption, a rail, and the number. The number
## is what the eye lands on, so it is set at the largest size on the panel and
## the caption at the smallest.
func _meter_row(
	parent: Node,
	caption: String,
	colour: Color,
	bar_height: float,
	value_size: int
) -> Array:
	var row := HBoxContainer.new()
	row.mouse_filter = Control.MOUSE_FILTER_IGNORE
	row.add_theme_constant_override("separation", 10)
	parent.add_child(row)

	var tag := HudTheme.label(row, caption, HudTheme.MICRO, HudTheme.DIM, 0.4, 1)
	tag.custom_minimum_size = Vector2(40.0, 0.0)
	tag.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	var meter := HudMeter.new()
	meter.accent = colour
	meter.custom_minimum_size = Vector2(0.0, bar_height)
	meter.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	meter.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	row.add_child(meter)

	var value := HudTheme.label(
		row, "0", value_size, HudTheme.WHITE, 0.25 if value_size >= HudTheme.BIG else 0.15, 0
	)
	value.custom_minimum_size = Vector2(86.0 if value_size >= HudTheme.BIG else 54.0, 0.0)
	value.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	value.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	return [meter, value]


func _build_weapons() -> void:
	_weapons = HudFrame.new()
	_weapons.accent = HudTheme.AMBER
	_weapons.rule_length = 0.0
	add_child(_weapons)
	_weapons.place(
		Control.PRESET_BOTTOM_RIGHT,
		Vector2(-HudTheme.MARGIN - PANEL_WEAPONS.x, -HudTheme.MARGIN - PANEL_WEAPONS.y),
		PANEL_WEAPONS
	)
	_weapons.set_meta(
		&"hud_rest",
		Vector2(-HudTheme.MARGIN - PANEL_WEAPONS.x, -HudTheme.MARGIN - PANEL_WEAPONS.y)
	)
	_weapons.header("VK-40 AUTOCANNON", "KINETIC")
	_weapons.column.add_theme_constant_override("separation", 5)

	var top := HBoxContainer.new()
	top.mouse_filter = Control.MOUSE_FILTER_IGNORE
	top.add_theme_constant_override("separation", 6)
	_weapons.column.add_child(top)

	_ammo = HudTheme.label(top, "42", HudTheme.BIG, HudTheme.WHITE, 0.3, 0)
	_ammo.vertical_alignment = VERTICAL_ALIGNMENT_BOTTOM
	_magazine = HudTheme.label(top, "/ 42", HudTheme.LABEL, HudTheme.FAINT, 0.3, 1)
	_magazine.vertical_alignment = VERTICAL_ALIGNMENT_BOTTOM
	_magazine.size_flags_horizontal = Control.SIZE_EXPAND_FILL

	_weapon_state = HudTheme.label(top, "READY", HudTheme.LABEL, HudTheme.CYAN, 0.4, 2)
	_weapon_state.vertical_alignment = VERTICAL_ALIGNMENT_BOTTOM
	_weapon_state.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT

	_reload = HudMeter.new()
	_reload.accent = HudTheme.AMBER
	_reload.ghost = false
	_reload.custom_minimum_size = Vector2(0.0, 4.0)
	_reload.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_weapons.column.add_child(_reload)

	_reload_caption = HudTheme.label(_weapons.column, "", HudTheme.MICRO, HudTheme.AMBER, 0.4, 2)
	_reload_caption.custom_minimum_size = Vector2(0.0, 12.0)

	var rule := HudTheme.rule(_weapons.column, HudTheme.HAIRLINE)
	rule.custom_minimum_size = Vector2(0.0, 1.0)

	_missile_state = _weapon_row(_weapons.column, "RMB", MISSILE)
	_blade_state = _weapon_row(_weapons.column, "V", BLADE)
	_repair_state = _weapon_row(_weapons.column, "F", "FIELD REPAIR")
	_weapons.fit()


func _weapon_row(parent: Node, key: String, name: String) -> Label:
	var row := HBoxContainer.new()
	row.mouse_filter = Control.MOUSE_FILTER_IGNORE
	row.add_theme_constant_override("separation", 8)
	parent.add_child(row)

	var tag := HudTheme.label(row, key, HudTheme.MICRO, HudTheme.AMBER, 0.5, 1)
	tag.custom_minimum_size = Vector2(28.0, 0.0)
	tag.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	var title := HudTheme.label(row, name, HudTheme.LABEL, HudTheme.DIM, 0.2, 1)
	title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	title.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	var state := HudTheme.label(row, "—", HudTheme.LABEL, HudTheme.FAINT, 0.4, 1)
	state.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	state.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	return state


func _build_boss() -> void:
	_boss = HudFrame.new()
	_boss.accent = HudTheme.RED
	_boss.rule_length = 0.0
	add_child(_boss)
	# Anchored from the centre explicitly rather than through `place`, because a
	# centre anchor resolves against the parent's width and the panel has to sit
	# on the screen's centre line whatever that width is.
	_boss.set_anchors_preset(Control.PRESET_CENTER_TOP)
	_boss.offset_left = -PANEL_BOSS.x * 0.5
	_boss.offset_right = PANEL_BOSS.x * 0.5
	_boss.offset_top = HudTheme.MARGIN
	_boss.offset_bottom = HudTheme.MARGIN + PANEL_BOSS.y
	_boss.header(BOSS_NAME, "")
	_boss.set_title_colour(HudTheme.RED)
	_boss.column.add_theme_constant_override("separation", 5)

	var row := HBoxContainer.new()
	row.mouse_filter = Control.MOUSE_FILTER_IGNORE
	row.add_theme_constant_override("separation", 10)
	_boss.column.add_child(row)

	_boss_meter = HudMeter.new()
	_boss_meter.accent = HudTheme.RED
	_boss_meter.segments = 20
	_boss_meter.notches = PackedFloat32Array([BOSS_PHASE_2, BOSS_PHASE_3])
	_boss_meter.custom_minimum_size = Vector2(0.0, 12.0)
	_boss_meter.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_boss_meter.size_flags_vertical = Control.SIZE_SHRINK_CENTER
	row.add_child(_boss_meter)

	_boss_value = HudTheme.label(row, "100%", HudTheme.LABEL, HudTheme.WHITE, 0.3, 1)
	_boss_value.custom_minimum_size = Vector2(46.0, 0.0)
	_boss_value.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	_boss_value.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	_boss_phase = HudTheme.label(_boss.column, "", HudTheme.MICRO, HudTheme.FAINT, 0.4, 2)
	_boss.visible = false


## The two pieces of text that report an event rather than a state: the damage
## just taken, under the vitals, and what a kill paid, beside the sight.
func _build_floats() -> void:
	_damage_text = HudTheme.label(self, "", HudTheme.LABEL, HudTheme.RED, 0.5, 1)
	HudTheme.shadow(_damage_text)
	_anchor_offsets(
		_damage_text, Control.PRESET_BOTTOM_LEFT,
		HudTheme.MARGIN + 4.0, -HudTheme.MARGIN - PANEL_VITALS.y - 22.0,
		HudTheme.MARGIN + 164.0, -HudTheme.MARGIN - PANEL_VITALS.y - 2.0
	)
	_damage_text.modulate.a = 0.0

	_score_text = HudTheme.label(self, "", HudTheme.LABEL, HudTheme.AMBER, 0.5, 2)
	HudTheme.shadow(_score_text)
	_anchor_offsets(_score_text, Control.PRESET_CENTER, 52.0, 88.0, 252.0, 110.0)
	_score_text.modulate.a = 0.0

	_stagger = HudTheme.label(self, "", HudTheme.BODY, HudTheme.RED, 0.6, 4)
	HudTheme.shadow(_stagger)
	_anchor_offsets(_stagger, Control.PRESET_CENTER, -180.0, -116.0, 180.0, -94.0)
	_stagger.horizontal_alignment = HORIZONTAL_ALIGNMENT_CENTER


func _anchor_offsets(
	node: Control, preset: int, left: float, top: float, right: float, bottom: float
) -> void:
	node.set_anchors_preset(preset)
	node.offset_left = left
	node.offset_top = top
	node.offset_right = right
	node.offset_bottom = bottom


# ── phase ───────────────────────────────────────────────────────────────────

func _enter_phase(phase: String) -> void:
	_phase = phase
	# Every string on screen is treated as stale: the panels are about to be
	# rebuilt from the game's state, and a cached value from the last mission
	# would be a lie until it happened to change.
	_last_health_int = -1
	_last_stability_int = -1
	_last_energy_int = -1
	_last_ammo = -1
	_last_kills = -1
	_last_score = -1
	_last_enemies = -1
	_last_seconds = -1
	_last_objective = ""
	_last_hint = ""
	_last_vitals_state = ""
	_last_weapon_state = ""
	_last_boss_phase = ""
	_critical = 0.0
	_damage_flash = 0.0

	match phase:
		"playing":
			_screens.hide_all()
			_set_live_visible(true)
			_play_entrance()
			_last_magazine = -1
		"ready":
			_set_live_visible(false)
			_screens.show_briefing(game.control_help(), game.objective())
		"paused":
			_set_live_visible(false)
			_screens.show_pause(_pause_values())
		"results":
			_set_live_visible(false)
			_show_results()


func _set_live_visible(on: bool) -> void:
	_mission.visible = on
	_clock_frame.visible = on
	_vitals.visible = on
	_weapons.visible = on
	_reticle.live = on
	_reticle.visible = on
	if not on:
		_boss.visible = false


## The panels arrive rather than appear: a short fade with a few pixels of travel
## in from the edge each one is anchored to. The travel is done on the offsets,
## because an anchored control's rectangle is derived from them and assigning
## `position` on a bottom-anchored panel puts it off the top of the screen.
func _play_entrance() -> void:
	if _entrance != null and _entrance.is_valid():
		_entrance.kill()
	if _motion_off:
		for panel in _live:
			panel.modulate.a = 1.0
			_slide(panel.get_meta(&"hud_rest", Vector2.ZERO).x, panel)
		_reticle.modulate.a = 1.0
		return
	_entrance = create_tween()
	_entrance.set_parallel(true)
	for panel in _live:
		var rest: Vector2 = panel.get_meta(&"hud_rest", Vector2.ZERO)
		var travel := -16.0 if rest.x < 0.0 else 16.0
		panel.modulate.a = 0.0
		_slide(rest.x + travel, panel)
		_entrance.tween_method(
			_slide.bind(panel), rest.x + travel, rest.x, 0.32
		).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
		_entrance.tween_property(panel, "modulate:a", 1.0, 0.24)
	_reticle.modulate.a = 0.0
	_entrance.tween_property(_reticle, "modulate:a", 1.0, 0.30).set_delay(0.06)


## Bound arguments arrive after the tweened value, so the value comes first.
func _slide(x: float, panel: Control) -> void:
	panel.offset_left = x
	panel.offset_right = x + panel.size.x


# ── the live readout ────────────────────────────────────────────────────────

func _update_live() -> void:
	_update_vitals()
	_update_weapons()
	_update_mission()
	_update_boss()
	_update_reticle()


func _update_vitals() -> void:
	var health: float = game.health()
	var health_max: float = maxf(game.health_max(), 1.0)
	_hull.set_fraction(health / health_max)
	var whole := roundi(health)
	if whole != _last_health_int:
		_last_health_int = whole
		HudTheme.write(_hull_value, str(whole))

	var stability: float = game.stability()
	_stability.set_fraction(stability / maxf(game.stability_max(), 1.0))
	var balance := roundi(stability)
	if balance != _last_stability_int:
		_last_stability_int = balance
		HudTheme.write(_stability_value, str(balance))

	var energy: float = game.energy()
	_energy.set_fraction(energy / maxf(game.energy_max(), 1.0))
	var charge := roundi(energy)
	if charge != _last_energy_int:
		_last_energy_int = charge
		HudTheme.write(_energy_value, str(charge))

	# One line that says what the frame is doing about its own condition, in the
	# order the player would care about it.
	var state := ""
	var colour := HudTheme.DIM
	if game.staggered():
		state = "FRAME STAGGERED // CONTROLS DEGRADED"
		colour = HudTheme.RED
	elif health / health_max < 0.16:
		state = "HULL CRITICAL // REPAIR OR BREAK CONTACT"
		colour = HudTheme.RED
	elif game.repairing():
		state = "FIELD REPAIR IN PROGRESS"
		colour = HudTheme.AMBER
	elif game.repairs() > 0:
		state = "NOMINAL // %d FIELD REPAIR%s STOWED" % [
			game.repairs(), "" if game.repairs() == 1 else "S",
		]
	if state != _last_vitals_state:
		_last_vitals_state = state
		HudTheme.write(_vitals_state, state)
		HudTheme.tint(_vitals_state, colour)


func _update_weapons() -> void:
	var ammo: int = game.ammo()
	if ammo != _last_ammo:
		# The sight blooms when a round leaves the barrel. Ammo going down is the
		# only weapon event the HUD is told about, and it is enough.
		if _last_ammo > ammo:
			_reticle.recoil(0.35 + 0.25 * float(mini(_last_ammo - ammo, 3)))
		_last_ammo = ammo
		HudTheme.write(_ammo, str(ammo))
	var magazine: int = game.magazine()
	if magazine != _last_magazine:
		_last_magazine = magazine
		HudTheme.write(_magazine, "/ %d" % magazine)

	var reloading: bool = game.reloading()
	# The rail is only there while it means something. An empty rail with two
	# hairlines reads as a dashed rule, which is to say as noise.
	_reload.visible = reloading
	_reload.set_fraction(game.reload_progress() if reloading else 0.0)

	var state := ""
	var colour := HudTheme.CYAN
	if reloading:
		state = "RELOADING"
		colour = HudTheme.AMBER
		HudTheme.write(
			_reload_caption, "RELOADING  %d%%" % roundi(game.reload_progress() * 100.0)
		)
	elif ammo == 0:
		state = "EMPTY"
		colour = HudTheme.RED
		HudTheme.write(_reload_caption, "MAGAZINE EMPTY  //  R TO RELOAD")
	else:
		state = "READY"
		HudTheme.write(_reload_caption, "")
	if state != _last_weapon_state:
		_last_weapon_state = state
		HudTheme.write(_weapon_state, state)
		HudTheme.tint(_weapon_state, colour)
	HudTheme.tint(_ammo, HudTheme.AMBER if reloading else HudTheme.WHITE)

	# Missiles: not ready because there is nothing locked is a different message
	# from not ready because the pod is still cycling, and the player can act on
	# one of them.
	if not game.missiles_locked():
		_weapon_label(_missile_state, "NO LOCK", HudTheme.DIM)
	elif game.missiles_ready():
		_weapon_label(_missile_state, "READY", HudTheme.CYAN)
	else:
		_weapon_label(_missile_state, "CYCLING", HudTheme.AMBER)

	match game.blade_phase():
		"windup":
			_weapon_label(_blade_state, "WINDUP", HudTheme.AMBER)
		"active":
			_weapon_label(_blade_state, "STRIKE", HudTheme.WHITE)
		"recovery":
			_weapon_label(_blade_state, "RECOVER", HudTheme.DIM)
		_:
			if game.blade_ready():
				_weapon_label(_blade_state, "READY", HudTheme.CYAN)
			else:
				_weapon_label(_blade_state, "CYCLING", HudTheme.AMBER)

	if game.repairing():
		_weapon_label(_repair_state, "REPAIRING", HudTheme.AMBER)
	elif game.repairs() > 0:
		_weapon_label(_repair_state, "x%d STOWED" % game.repairs(), HudTheme.CYAN)
	else:
		_weapon_label(_repair_state, "EXPENDED", HudTheme.FAINT)


func _weapon_label(label: Label, text: String, colour: Color) -> void:
	HudTheme.write(label, text)
	HudTheme.tint(label, colour)


## The mission's own phase identifiers, written the way a pilot would read them.
## The simulation's are `wave1`, `gap2`, `boss`; a line of lower case with a digit
## stuck to it is a debugging aid, not a status.
func _phase_name(phase: String) -> String:
	match phase:
		"intro":
			return "DEPLOYMENT"
		"wave1":
			return "WAVE 1"
		"gap1", "gap2":
			return "REGROUPING"
		"wave2":
			return "WAVE 2"
		"boss":
			return "PATRIARCH ENGAGED"
		"victory":
			return "MISSION COMPLETE"
		"defeat":
			return "FRAME LOST"
		_:
			return phase.to_upper()


func _update_mission() -> void:
	var enemies: int = game.enemies_left()
	var phase: String = _phase_name(game.mission_phase())
	if enemies != _last_enemies or phase != _last_mission_phase:
		_last_enemies = enemies
		_last_mission_phase = phase
		HudTheme.write(
			_mission_line,
			"ASHEN YARD  //  %s  //  HOSTILES %02d" % [phase, enemies]
		)

	var objective: String = game.objective()
	if objective != _last_objective:
		_last_objective = objective
		HudTheme.write(_objective, objective)
		_flash_objective()

	var hint: String = game.hint()
	if hint != _last_hint:
		_last_hint = hint
		HudTheme.write(_hint, hint)
		_fade_hint(not hint.is_empty())

	var seconds: int = int(game.elapsed())
	if seconds != _last_seconds:
		_last_seconds = seconds
		HudTheme.write(_clock, HudTheme.clock(game.elapsed()))

	var kills: int = game.kills()
	var score: int = game.score()
	if kills != _last_kills or score != _last_score:
		if kills > _last_kills and _last_kills >= 0:
			_show_score(score - _last_score)
		_last_kills = kills
		_last_score = score
		HudTheme.write(
			_tally, "%02d KILLS   %s PTS" % [kills, HudTheme.thousands(score)]
		)


func _update_boss() -> void:
	var present: bool = game.boss_present()
	if present != _last_boss:
		_last_boss = present
		if present:
			_show_boss()
		else:
			_boss.visible = false
	if not present:
		return
	var fraction := clampf(
		game.boss_health() / maxf(game.boss_health_max(), 1.0), 0.0, 1.0
	)
	_boss_meter.set_fraction(fraction)
	HudTheme.write(_boss_value, "%d%%" % roundi(fraction * 100.0))
	var phase := ""
	if fraction > BOSS_PHASE_2:
		phase = "PHASE I  //  HUNTING"
	elif fraction > BOSS_PHASE_3:
		phase = "PHASE II  //  PRESSING"
	else:
		phase = "PHASE III  //  DESPERATE"
	if phase != _last_boss_phase:
		_last_boss_phase = phase
		HudTheme.write(_boss_phase, phase)
		HudTheme.tint(
			_boss_phase,
			HudTheme.FAINT if fraction > BOSS_PHASE_2 else HudTheme.RED
		)


func _show_boss() -> void:
	_boss.visible = true
	_last_boss_phase = ""
	if _motion_off:
		_boss.modulate.a = 1.0
		return
	_boss.modulate.a = 0.0
	var tween := create_tween()
	tween.set_parallel(true)
	tween.tween_property(_boss, "modulate:a", 1.0, 0.28)
	tween.tween_property(_boss, "offset_top", HudTheme.MARGIN, 0.32).from(
		HudTheme.MARGIN - 18.0
	).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)


func _update_reticle() -> void:
	_reticle.speed = game.speed()
	_reticle.altitude = game.altitude()
	_reticle.grounded = game.grounded()
	_reticle.boosting = game.assault_boosting()
	_reticle.staggered = game.staggered()
	_reticle.reloading = game.reloading()
	_reticle.reload_progress = game.reload_progress()

	var name: String = game.lock_name()
	var locked: bool = game.missiles_locked() and not name.is_empty()
	_reticle.has_lock = false
	if locked:
		var camera := get_viewport().get_camera_3d()
		var world: Vector3 = game.lock_world_position()
		if camera != null and world != Vector3.ZERO and not camera.is_position_behind(world):
			_reticle.lock_point = camera.unproject_position(world)
			_reticle.lock_name = name
			_reticle.lock_distance = game.lock_distance()
			_reticle.lock_health = game.lock_health()
			_reticle.has_lock = true

	var staggered: bool = game.staggered()
	if staggered and not _last_staggered:		# Losing the frame's footing is worth more than a line of text: the sight
		# splays and shakes, the wash comes up, and the hull bar is told.
		_damage_flash = maxf(_damage_flash, 1.25)
		_vitals.flash(1.0)
	_staggered_state(staggered)
	_last_staggered = staggered


func _staggered_state(staggered: bool) -> void:
	if staggered:
		HudTheme.write(_stagger, "FRAME STAGGERED")
		_stagger.visible = true
		if not _motion_off:
			_stagger.modulate.a = 0.35 + 0.65 * absf(sin(Time.get_ticks_msec() * 0.006))
		else:
			_stagger.modulate.a = 1.0
	else:
		_stagger.visible = false


# ── the screens ─────────────────────────────────────────────────────────────

func _pause_values() -> PackedStringArray:
	return PackedStringArray([
		"%d / %d" % [roundi(game.health()), roundi(game.health_max())],
		"%d STOWED" % game.repairs(),
		"%d" % game.kills(),
		HudTheme.clock(game.elapsed()),
	])


func _show_results() -> void:
	var won: bool = game.won()
	var damage := roundi(game.damage_taken())
	var note := "FRAME LOST"
	if won and damage < 350:
		note = "CLEAN RUN // HULL HELD"
	elif won and game.repairs_used() == 0:
		note = "NO FIELD REPAIRS USED"
	elif won:
		note = "OPERATION COMPLETE"
	elif damage > 900:
		note = "OVERWHELMED // HULL FAILURE"
	_screens.show_results(
		won,
		_verdict(won),
		HudTheme.thousands(game.score()),
		PackedStringArray([
			HudTheme.clock(game.elapsed()),
			"%d" % game.kills(),
			"%d" % game.shots_fired(),
			"%d%%" % roundi(game.accuracy()),
			"%d" % damage,
			"%d" % game.repairs_used(),
		]),
		note
	)


func _verdict(won: bool) -> String:
	if won:
		return (
			"The Patriarch is down and the yard is quiet. Ashen Yard is ours, and"
			+ " the frame came home under its own power."
		)
	return (
		"The frame is down in the yard. The screen is still standing, and the"
		+ " Patriarch never had to commit."
	)


# ── events ──────────────────────────────────────────────────────────────────

func _update_damage(delta: float) -> void:
	var health: float = game.health()
	if _phase == "playing" and health < _last_health - 0.5:
		var loss := roundi(_last_health - health)
		_damage_flash = 1.0
		_vitals.flash(1.0)
		_show_damage(loss)
		_point_at_threat()
	_last_health = health

	var fraction := 1.0
	var health_max: float = maxf(game.health_max(), 1.0)
	if health_max > 0.0:
		fraction = health / health_max
	if _phase == "playing" and fraction < 0.25:
		_critical = minf(_critical + delta * 1.6, 1.0)
	else:
		_critical = maxf(_critical - delta * 1.2, 0.0)
	_damage_flash = maxf(_damage_flash - delta * 1.8, 0.0)

	var level := maxf(_damage_flash, _critical * 0.34)
	if absf(level - _vignette_level) > 0.004:
		_vignette_level = level
		# The texture is already at full strength at the corners, so the level
		# caps how far a single hit is allowed to push it.
		_vignette.modulate.a = clampf(level, 0.0, 1.0) * 0.72


func _show_damage(loss: int) -> void:
	HudTheme.write(_damage_text, "-%d" % loss)
	if _motion_off:
		_damage_text.modulate.a = 1.0
		return
	if _damage_tween != null and _damage_tween.is_valid():
		_damage_tween.kill()
	_damage_text.modulate.a = 1.0
	var height := _damage_text.offset_bottom - _damage_text.offset_top
	var rest := _damage_text.offset_top
	_damage_tween = create_tween()
	_damage_tween.set_parallel(true)
	_damage_tween.tween_method(
		_travel.bind(_damage_text, height), rest, rest - 18.0, 0.7
	).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	_damage_tween.tween_property(_damage_text, "modulate:a", 0.0, 0.8).set_delay(0.3)


func _show_score(points: int) -> void:
	if points <= 0:
		return
	HudTheme.write(_score_text, "+ %s" % HudTheme.thousands(points))
	if _motion_off:
		_score_text.modulate.a = 0.0
		return
	if _score_tween != null and _score_tween.is_valid():
		_score_tween.kill()
	_score_text.modulate.a = 1.0
	var height := _score_text.offset_bottom - _score_text.offset_top
	var rest := _score_text.offset_top
	_score_tween = create_tween()
	_score_tween.set_parallel(true)
	_score_tween.tween_method(
		_travel.bind(_score_text, height), rest, rest - 20.0, 0.9
	).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
	_score_tween.tween_property(_score_text, "modulate:a", 0.0, 0.9).set_delay(0.3)


func _travel(value: float, node: Control, height: float) -> void:
	node.offset_top = value
	node.offset_bottom = value + height


func _flash_objective() -> void:
	if _motion_off:
		return
	if _objective_tween != null and _objective_tween.is_valid():
		_objective_tween.kill()
	_objective.modulate = Color(1.7, 1.4, 0.9)
	_objective_tween = create_tween()
	_objective_tween.tween_property(_objective, "modulate", Color.WHITE, 0.55)


func _fade_hint(shown: bool) -> void:
	if _motion_off:
		_hint.modulate.a = 1.0 if shown else 0.0
		return
	if _hint_tween != null and _hint_tween.is_valid():
		_hint_tween.kill()
	_hint_tween = create_tween()
	_hint_tween.tween_property(_hint, "modulate:a", 1.0 if shown else 0.0, 0.25)


## Points the sight's threat arc at the nearest live contact. The simulation does
## not say who fired, so this is what the HUD can honestly know: something is out
## there, in that direction, and it is the closest thing to the frame. A contact
## behind the camera puts the arc at the bottom of the sight, which reads as
## "behind you" rather than as a bearing that is not on screen.
func _point_at_threat() -> void:
	var camera := get_viewport().get_camera_3d()
	if camera == null or _player_rig == null or _contacts.is_empty():
		return
	var from := _player_rig.global_position
	var nearest := _contacts[0]
	var best := from.distance_squared_to(nearest)
	for i in range(1, _contacts.size()):
		var distance := from.distance_squared_to(_contacts[i])
		if distance < best:
			best = distance
			nearest = _contacts[i]
	var local := camera.global_transform.affine_inverse() * nearest
	_reticle.threat_bearing = PI * 0.5 if local.z > 0.0 else atan2(local.x, -local.z)
	_reticle.threat = 1.0


# ── the world outside the simulation ────────────────────────────────────────

## The contact readout needs the positions of the hostiles, and the only thing
## that knows them is the scene the simulation is rendered into. Every mech is a
## rig: a plain `Node3D` child of the game node, unnamed, with the player's built
## first and the hostiles after it in roster order. A dead hostile keeps its rig
## and is hidden rather than freed, and the scene's own containers — the arena,
## the effects pool — are named, which is what separates them from a rig.
##
## The count of visible rigs then has to be the player plus the hostiles the
## simulation reports. When it is not, the readout is not drawn at all. That
## check is the whole reason this is allowed to exist: a wrong contact is worse
## than no contact, and the alternative — trusting whichever child looks like a
## mech — is how a HUD ends up pointing at a crate.
func _sample_world() -> void:
	var reduce := _prefers_less_motion()
	if reduce != _motion_off:
		_motion_off = reduce
		_reticle.reduced_motion = reduce
		_screens.set_reduced_motion(reduce)

	_contacts.resize(0)
	_player_rig = null
	var visible_rigs := 0
	for child in game.get_children():
		if not _is_rig(child):
			continue
		var rig := child as Node3D
		if not rig.visible:
			continue
		visible_rigs += 1
		if _player_rig == null:
			_player_rig = rig
			continue
		_contacts.append(rig.global_position + Vector3.UP * 2.4)

	_contacts_trusted = (
		_player_rig != null
		and visible_rigs == game.enemies_left() + 1
	)
	_reticle.contacts = _contacts
	_reticle.contacts_trusted = _contacts_trusted


func _is_rig(node: Node) -> bool:
	return node.get_class() == "Node3D" and String(node.name).begins_with("@")


## Reduced motion, asked of the system. Godot 4.7 exposes the platform's own
## preference, and a project setting and an environment variable are offered as
## well, because a capture harness has no way to change an operating system
## setting and a reviewer still needs to see the still version of a screen.
func _prefers_less_motion() -> bool:
	var reduced := bool(ProjectSettings.get_setting("accessibility/reduced_motion", false))
	if DisplayServer.has_method("accessibility_should_reduce_animation"):
		reduced = reduced or bool(DisplayServer.call("accessibility_should_reduce_animation"))
	if OS.has_environment("ASHFRAME_REDUCED_MOTION"):
		reduced = OS.get_environment("ASHFRAME_REDUCED_MOTION") != "0"
	return reduced
