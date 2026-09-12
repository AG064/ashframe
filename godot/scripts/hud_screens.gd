## ASHFRAME — the three screens the player reads on purpose.
##
## The briefing, the pause card and the after-action report are not the HUD with
## the middle covered up. They are the only screens in the game where nobody is
## shooting, so they carry the operation's voice: a rule across the top, a title
## block on the left, a reference column on the right, and a footer that says
## what to press.
##
## The cards are laid out rather than centred. A column of centred text is what a
## prototype does. A briefing has an eyebrow, a title, a rule, a paragraph and a
## data strip, and every one of them starts at the same left edge.
##
## Everything here is positioned by its anchor offsets. A control's `position` is
## the rectangle it ended up with, not something to assign, and a screen placed
## that way ends up off the edge of the viewport the moment its anchor is not the
## top left corner.

class_name HudScreens
extends Control

const MARGIN := 26.0
const TITLE_TOP := 160.0
const TITLE_W := 660.0
const TITLE_H := 300.0
const REFERENCE_W := 592.0
const REFERENCE_H := 428.0
const STRIP_LIFT := 96.0

## The operation as the briefing tells it. Fiction — but the mission's own first
## task is printed under it from the game's getter, so the two cannot disagree
## about what the player is being asked to do.
const BRIEF := (
	"A skirmisher screen is holding the yard, and a siege pair is dug in behind"
	+ " it. Break the screen, survive the artillery, and bring the Patriarch"
	+ " down before it commits."
)

## The loadout, named as the simulation names it. A briefing that describes
## weapons the frame does not carry is worse than no briefing.
const LOADOUT := [
	["LMB", "VK-40 AUTOCANNON", "42-round magazine"],
	["RMB", "HR-6 MISSILE POD", "six rounds · lock required"],
	["V", "KS-9 ARC BLADE", "close quarters · builds stagger"],
]

var _briefing: Control
var _pause: Control
var _results: Control

var _brief_objective: Label
var _brief_footer: Label
var _brief_controls: VBoxContainer
var _brief_controls_built: bool = false
var _brief_movers: Array[Control] = []

var _pause_values: Array[Label] = []
var _pause_footer: Label
var _pause_card: Control

var _result_title: Label
var _result_eyebrow: Label
var _result_verdict: Label
var _result_assessment: Label
var _result_score: Label
var _result_values: Array[Label] = []
var _result_footer: Label
var _result_card: Control

var _entrance: Tween
var _blink: Tween
var _motion_off: bool = false


func _init() -> void:
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	set_anchors_preset(Control.PRESET_FULL_RECT)
	_briefing = _screen()
	_build_briefing()
	_pause = _screen()
	_build_pause()
	_results = _screen()
	_build_results()
	hide_all()


# ── construction ────────────────────────────────────────────────────────────

func _screen() -> Control:
	var root := Control.new()
	root.set_anchors_preset(Control.PRESET_FULL_RECT)
	root.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(root)
	return root


## Position a control by its anchor offsets and remember where it belongs, so an
## entrance can move it and put it back.
func _place(node: Control, preset: int, offset: Vector2, box: Vector2) -> void:
	node.set_anchors_preset(preset)
	node.offset_left = offset.x
	node.offset_top = offset.y
	node.offset_right = offset.x + box.x
	node.offset_bottom = offset.y + box.y
	node.custom_minimum_size = box
	node.set_meta(&"hud_rest", offset)


func _block(parent: Control, preset: int, offset: Vector2, box: Vector2) -> Control:
	var node := Control.new()
	node.mouse_filter = Control.MOUSE_FILTER_IGNORE
	parent.add_child(node)
	_place(node, preset, offset, box)
	return node


func _label_at(
	parent: Control,
	text: String,
	size: int,
	colour: Color,
	preset: int,
	offset: Vector2,
	box: Vector2,
	bold: float = 0.4,
	tracking: int = 3
) -> Label:
	var node := HudTheme.label(parent, text, size, colour, bold, tracking)
	_place(node, preset, offset, box)
	node.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	return node


func _column(parent: Control, preset: int, offset: Vector2, box: Vector2) -> VBoxContainer:
	var host := _block(parent, preset, offset, box)
	var column := VBoxContainer.new()
	column.set_anchors_preset(Control.PRESET_FULL_RECT)
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	column.add_theme_constant_override("separation", int(HudTheme.ROW))
	host.add_child(column)
	return column


## A wash for the screen behind the cards. The briefing gets a horizontal one, so
## the yard stays visible on the right of the operation's name; the centred cards
## get a vertical one, because there is nothing behind them worth seeing.
func _wash(parent: Control, horizontal: bool, strength: float) -> void:
	var gradient := Gradient.new()
	if horizontal:
		gradient.offsets = PackedFloat32Array([0.0, 0.44, 1.0])
		gradient.colors = PackedColorArray([
			Color(0.008, 0.012, 0.018, strength),
			Color(0.008, 0.012, 0.018, strength * 0.62),
			Color(0.008, 0.012, 0.018, strength * 0.18),
		])
	else:
		gradient.offsets = PackedFloat32Array([0.0, 1.0])
		gradient.colors = PackedColorArray([
			Color(0.008, 0.012, 0.018, strength * 0.82),
			Color(0.008, 0.012, 0.018, strength),
		])
	_wash_rect(parent, gradient, true)


## A band across the top or the bottom that fades into the picture. It is what
## makes small capitals legible over a bright sky, and it frames the whole
## interface the way a letterbox frames a shot.
func _band(parent: Control, at_top: bool, depth: float, strength: float) -> void:
	var gradient := Gradient.new()
	var dark := Color(0.006, 0.010, 0.015, strength)
	var clear := Color(0.006, 0.010, 0.015, 0.0)
	if at_top:
		gradient.offsets = PackedFloat32Array([0.0, 0.65, 1.0])
		gradient.colors = PackedColorArray([dark, dark, clear])
	else:
		gradient.offsets = PackedFloat32Array([0.0, 0.35, 1.0])
		gradient.colors = PackedColorArray([clear, dark, dark])
	var node := _wash_rect(parent, gradient, false)
	node.set_anchors_preset(
		Control.PRESET_TOP_WIDE if at_top else Control.PRESET_BOTTOM_WIDE
	)
	node.offset_left = 0.0
	node.offset_right = 0.0
	node.offset_top = 0.0 if at_top else -depth
	node.offset_bottom = depth if at_top else 0.0


func _wash_rect(parent: Control, gradient: Gradient, full: bool) -> TextureRect:
	var texture := GradientTexture2D.new()
	texture.gradient = gradient
	texture.fill = GradientTexture2D.FILL_LINEAR
	texture.fill_from = Vector2(0.0, 0.5) if full else Vector2(0.5, 0.0)
	texture.fill_to = Vector2(1.0, 0.5) if full else Vector2(0.5, 1.0)
	texture.width = 256
	texture.height = 8

	var rect := TextureRect.new()
	rect.texture = texture
	rect.stretch_mode = TextureRect.STRETCH_SCALE
	rect.mouse_filter = Control.MOUSE_FILTER_IGNORE
	parent.add_child(rect)
	return rect


func _eyebrow(parent: Node, text: String, colour: Color) -> Label:
	return HudTheme.label(parent, text, HudTheme.MICRO, colour, 0.45, 3)


func _build_briefing() -> void:
	_wash(_briefing, true, 0.95)
	_band(_briefing, true, 132.0, 0.92)
	_band(_briefing, false, 168.0, 0.94)

	_label_at(
		_briefing, "ASHFRAME // FIELD OPERATING SYSTEM", HudTheme.MICRO,
		HudTheme.DIM, Control.PRESET_TOP_LEFT,
		Vector2(MARGIN, 34.0), Vector2(420.0, 20.0)
	)
	_label_at(
		_briefing, "ASHTECH INDUSTRIES · ASHEN YARD · 04:40 LOCAL", HudTheme.MICRO,
		HudTheme.DIM, Control.PRESET_TOP_RIGHT,
		Vector2(-(560.0 + MARGIN), 34.0), Vector2(560.0, 20.0)
	).horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT

	var title := _column(
		_briefing, Control.PRESET_TOP_LEFT,
		Vector2(MARGIN, TITLE_TOP), Vector2(TITLE_W, TITLE_H)
	)
	_brief_movers.append(title.get_parent() as Control)
	_eyebrow(title, "OPERATION 01 // DEPLOYMENT BRIEF", HudTheme.AMBER)
	var operation := HudTheme.label(
		title, "ASHEN YARD", HudTheme.HUGE, HudTheme.WHITE, 0.35, 4
	)
	operation.custom_minimum_size = Vector2(0.0, 60.0)
	var accent := HudTheme.rule(title, HudTheme.AMBER, 2.0)
	accent.custom_minimum_size = Vector2(148.0, 2.0)
	accent.size_flags_horizontal = Control.SIZE_SHRINK_BEGIN

	var prose := HudTheme.label(title, BRIEF, HudTheme.BODY, HudTheme.WHITE, 0.1, 0)
	prose.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	prose.custom_minimum_size = Vector2(TITLE_W - 80.0, 78.0)
	prose.size_flags_horizontal = Control.SIZE_SHRINK_BEGIN

	# The one line on this screen that comes from the game rather than from the
	# fiction, so a briefing cannot promise what the simulation will not ask for.
	HudTheme.rule(title, HudTheme.HAIRLINE)
	_brief_objective = HudTheme.label(title, "", HudTheme.LABEL, HudTheme.CYAN, 0.45, 2)

	# The facts sit on the bottom strip rather than under the title: a document
	# puts its reference data at the foot, and it stops the left column from
	# being top-heavy.
	var facts := HBoxContainer.new()
	facts.mouse_filter = Control.MOUSE_FILTER_IGNORE
	facts.add_theme_constant_override("separation", 52)
	_block(
		_briefing, Control.PRESET_BOTTOM_LEFT,
		Vector2(MARGIN, -(STRIP_LIFT + 74.0)), Vector2(TITLE_W, 46.0)
	).add_child(facts)
	_fact(facts, "WINDOW", "04:40")
	_fact(facts, "AREA", "THE YARD")
	_fact(facts, "FRAME", "ASHFRAME-01")
	_fact(facts, "REPAIRS", "2 STOWED")

	var card := PanelContainer.new()
	card.mouse_filter = Control.MOUSE_FILTER_IGNORE
	card.add_theme_stylebox_override(
		"panel", HudTheme.card(HudTheme.INK_DEEP, HudTheme.HAIRLINE, 1, 4)
	)
	_briefing.add_child(card)
	_place(
		card, Control.PRESET_TOP_RIGHT,
		Vector2(-(REFERENCE_W + MARGIN), TITLE_TOP), Vector2(REFERENCE_W, REFERENCE_H)
	)
	_brief_movers.append(card)

	_brief_controls = VBoxContainer.new()
	_brief_controls.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_brief_controls.add_theme_constant_override("separation", 7)
	card.add_child(_brief_controls)
	_eyebrow(_brief_controls, "CONTROL REFERENCE", HudTheme.AMBER)
	HudTheme.spacer(_brief_controls, 2.0)

	_wide_rule(_briefing, STRIP_LIFT, HudTheme.HAIRLINE)
	_label_at(
		_briefing, "FRAME ASHFRAME-01 // ALL SYSTEMS NOMINAL // 2 FIELD REPAIRS STOWED",
		HudTheme.MICRO, HudTheme.DIM, Control.PRESET_BOTTOM_LEFT,
		Vector2(MARGIN, -(STRIP_LIFT - 26.0)), Vector2(700.0, 20.0)
	)
	_brief_footer = _label_at(
		_briefing, "PRESS ENTER TO DEPLOY", HudTheme.LABEL, HudTheme.AMBER,
		Control.PRESET_BOTTOM_RIGHT,
		Vector2(-(380.0 + MARGIN), -(STRIP_LIFT - 26.0)), Vector2(380.0, 20.0), 0.6, 4
	)
	_brief_footer.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT


func _fact(parent: Node, name: String, value: String) -> void:
	var block := VBoxContainer.new()
	block.mouse_filter = Control.MOUSE_FILTER_IGNORE
	block.add_theme_constant_override("separation", 3)
	parent.add_child(block)
	_eyebrow(block, name, HudTheme.DIM)
	HudTheme.label(block, value, HudTheme.LABEL, HudTheme.WHITE, 0.4, 2)


func _build_pause() -> void:
	_wash(_pause, false, 0.90)

	var card := PanelContainer.new()
	card.mouse_filter = Control.MOUSE_FILTER_IGNORE
	card.add_theme_stylebox_override(
		"panel", HudTheme.card(HudTheme.INK_DEEP, HudTheme.HAIRLINE_HOT, 1, 4)
	)
	_pause.add_child(card)
	var box := Vector2(780.0, 308.0)
	_centre(card, box)
	_pause_card = card

	var column := VBoxContainer.new()
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	column.add_theme_constant_override("separation", 10)
	card.add_child(column)

	_eyebrow(column, "OPERATION SUSPENDED", HudTheme.AMBER)
	HudTheme.label(column, "PAUSED", HudTheme.HUGE, HudTheme.WHITE, 0.35, 4)
	var accent := HudTheme.rule(column, HudTheme.AMBER, 2.0)
	accent.custom_minimum_size = Vector2(120.0, 2.0)
	accent.size_flags_horizontal = Control.SIZE_SHRINK_BEGIN
	HudTheme.spacer(column, 4.0)

	var split := HBoxContainer.new()
	split.mouse_filter = Control.MOUSE_FILTER_IGNORE
	split.add_theme_constant_override("separation", 56)
	column.add_child(split)

	var status := VBoxContainer.new()
	status.mouse_filter = Control.MOUSE_FILTER_IGNORE
	status.add_theme_constant_override("separation", 6)
	status.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	split.add_child(status)
	_eyebrow(status, "FIELD STATUS", HudTheme.FAINT)
	_pause_values = _status_table(
		status, ["HULL", "REPAIRS", "HOSTILES DOWN", "TIME ON STATION"]
	)

	var keys := VBoxContainer.new()
	keys.mouse_filter = Control.MOUSE_FILTER_IGNORE
	keys.add_theme_constant_override("separation", 6)
	keys.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	split.add_child(keys)
	_eyebrow(keys, "CONTROLS", HudTheme.FAINT)
	_key_row(keys, "ESC", "resume the operation")
	_key_row(keys, "R", "restart from the briefing")

	HudTheme.spacer(column, 2.0)
	_pause_footer = HudTheme.label(
		column, "ESC TO RESUME", HudTheme.LABEL, HudTheme.CYAN, 0.6, 4
	)
	_right_align(_pause_footer)


func _build_results() -> void:
	_wash(_results, false, 0.92)
	_band(_results, true, 150.0, 0.85)
	_band(_results, false, 150.0, 0.85)

	var card := PanelContainer.new()
	card.mouse_filter = Control.MOUSE_FILTER_IGNORE
	card.add_theme_stylebox_override(
		"panel", HudTheme.card(HudTheme.INK_DEEP, HudTheme.HAIRLINE_HOT, 1, 4)
	)
	_results.add_child(card)
	var box := Vector2(980.0, 466.0)
	_centre(card, box)
	_result_card = card

	var column := VBoxContainer.new()
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	column.add_theme_constant_override("separation", 10)
	card.add_child(column)

	_result_eyebrow = _eyebrow(column, "AFTER ACTION // ASHEN YARD", HudTheme.AMBER)
	_result_title = HudTheme.label(
		column, "MISSION COMPLETE", HudTheme.HUGE, HudTheme.AMBER, 0.35, 4
	)
	_result_title.custom_minimum_size = Vector2(0.0, 58.0)
	var rule := HudTheme.rule(column, HudTheme.HAIRLINE_HOT, 1.0)
	rule.custom_minimum_size = Vector2(0.0, 1.0)
	HudTheme.spacer(column, 2.0)

	var split := HBoxContainer.new()
	split.mouse_filter = Control.MOUSE_FILTER_IGNORE
	split.add_theme_constant_override("separation", 60)
	column.add_child(split)

	var left := VBoxContainer.new()
	left.mouse_filter = Control.MOUSE_FILTER_IGNORE
	left.add_theme_constant_override("separation", 8)
	left.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	split.add_child(left)
	_eyebrow(left, "ASSESSMENT", HudTheme.FAINT)
	_result_verdict = HudTheme.label(left, "", HudTheme.BODY, HudTheme.WHITE, 0.1, 0)
	_result_verdict.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	_result_verdict.custom_minimum_size = Vector2(390.0, 74.0)
	HudTheme.spacer(left, 6.0)
	_eyebrow(left, "FINAL SCORE", HudTheme.FAINT)
	_result_score = HudTheme.label(left, "0", HudTheme.HUGE, HudTheme.WHITE, 0.35, 2)
	_result_assessment = HudTheme.label(left, "", HudTheme.MICRO, HudTheme.CYAN, 0.4, 2)

	var right := VBoxContainer.new()
	right.mouse_filter = Control.MOUSE_FILTER_IGNORE
	right.add_theme_constant_override("separation", 6)
	right.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	split.add_child(right)
	_eyebrow(right, "OPERATION RECORD", HudTheme.FAINT)
	_result_values = _status_table(
		right,
		[
			"TIME ON STATION", "HOSTILES DESTROYED", "ROUNDS FIRED",
			"ROUNDS THAT KILLED", "DAMAGE TAKEN", "REPAIRS USED",
		]
	)

	HudTheme.spacer(column, 2.0)
	HudTheme.rule(column, HudTheme.HAIRLINE_HOT)
	_result_footer = HudTheme.label(
		column, "PRESS ENTER FOR A NEW OPERATION", HudTheme.LABEL,
		HudTheme.AMBER, 0.6, 4
	)
	_right_align(_result_footer)


## Push a line to the right edge of the column it is in. The call to action on
## every screen sits over the column of values rather than under the prose, so
## the eye finds it in the same place each time.
func _right_align(label: Label) -> void:
	label.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT


## A hairline the full width of the screen, measured from the bottom edge so it
## stays where it belongs on a taller window.
func _wide_rule(parent: Control, lift: float, colour: Color) -> ColorRect:
	var line := ColorRect.new()
	line.color = colour
	line.mouse_filter = Control.MOUSE_FILTER_IGNORE
	parent.add_child(line)
	line.set_anchors_preset(Control.PRESET_BOTTOM_WIDE)
	line.offset_left = MARGIN
	line.offset_right = -MARGIN
	line.offset_top = -lift
	line.offset_bottom = -lift + 1.0
	return line


## Centre a card by offset rather than by position: a centre anchor resolves
## against the parent's width, and the card has to sit on the screen's centre
## line whatever that width turns out to be.
func _centre(card: Control, box: Vector2) -> void:
	_place(card, Control.PRESET_CENTER, Vector2(-box.x * 0.5, -box.y * 0.5), box)


## A label/value row with a hairline under it, which is what makes a list of
## numbers read as a record rather than as a paragraph.
func _status_table(parent: Node, labels: Array) -> Array[Label]:
	var values: Array[Label] = []
	for i in labels.size():
		var row := HBoxContainer.new()
		row.mouse_filter = Control.MOUSE_FILTER_IGNORE
		row.add_theme_constant_override("separation", 12)
		parent.add_child(row)
		var caption := HudTheme.label(row, labels[i], HudTheme.LABEL, HudTheme.DIM, 0.3, 2)
		caption.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		caption.vertical_alignment = VERTICAL_ALIGNMENT_BOTTOM
		var value := HudTheme.label(row, "—", HudTheme.BODY, HudTheme.WHITE, 0.35, 1)
		value.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
		value.vertical_alignment = VERTICAL_ALIGNMENT_BOTTOM
		values.append(value)
		if i < labels.size() - 1:
			HudTheme.rule(parent, HudTheme.HAIRLINE)
	return values


func _key_row(parent: Node, key: String, action: String) -> void:
	var row := HBoxContainer.new()
	row.mouse_filter = Control.MOUSE_FILTER_IGNORE
	row.add_theme_constant_override("separation", 16)
	parent.add_child(row)
	var key_label := HudTheme.label(row, key, HudTheme.LABEL, HudTheme.AMBER, 0.5, 2)
	key_label.custom_minimum_size = Vector2(76.0, 0.0)
	key_label.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	var action_label := HudTheme.label(row, action, HudTheme.LABEL, HudTheme.DIM, 0.15, 1)
	action_label.size_flags_horizontal = Control.SIZE_EXPAND_FILL


# ── what the HUD says to them ───────────────────────────────────────────────

func show_briefing(controls: String, objective: String) -> void:
	_brief_objective.text = "FIRST TASK  //  %s" % objective
	if not _brief_controls_built:
		_build_control_rows(controls)
		_brief_controls_built = true
	_swap(_briefing)
	_enter(_briefing, _brief_movers)
	_blink_footer(_brief_footer)


func show_pause(values: PackedStringArray) -> void:
	_set_values(_pause_values, values)
	_swap(_pause)
	_enter(_pause, [_pause_card] as Array[Control])
	_blink_footer(_pause_footer)


func show_results(
	won: bool,
	verdict: String,
	score: String,
	rows: PackedStringArray,
	note: String
) -> void:
	_result_title.text = "MISSION COMPLETE" if won else "FRAME LOST"
	_result_title.add_theme_color_override(
		"font_color", HudTheme.AMBER if won else HudTheme.RED
	)
	_result_eyebrow.text = (
		"AFTER ACTION // ASHEN YARD" if won
		else "AFTER ACTION // FRAME LOST IN THE YARD"
	)
	_result_verdict.text = verdict
	_result_score.text = score
	_result_assessment.text = note
	_set_values(_result_values, rows)
	_swap(_results)
	_enter(_results, [_result_card] as Array[Control])
	_blink_footer(_result_footer)


func hide_all() -> void:
	_halt()
	_briefing.visible = false
	_pause.visible = false
	_results.visible = false


func set_reduced_motion(reduced: bool) -> void:
	_motion_off = reduced
	if reduced and _blink != null and _blink.is_valid():
		_blink.kill()
		for footer in [_brief_footer, _pause_footer, _result_footer]:
			if footer != null:
				footer.modulate.a = 1.0


func _build_control_rows(help: String) -> void:
	# The game owns this text. The screen only decides that a binding is a key
	# and an action, set in two columns, so the controls cannot drift out of
	# the briefing.
	var groups := help.split("\n", false)
	for g in groups.size():
		var pairs := groups[g].split("   ", false)
		for pair in pairs:
			var trimmed := pair.strip_edges()
			var space := trimmed.find(" ")
			if space <= 0:
				continue
			_key_row(
				_brief_controls,
				trimmed.substr(0, space).to_upper(),
				trimmed.substr(space + 1)
			)
		if g < groups.size() - 1:
			HudTheme.spacer(_brief_controls, 2.0)
	# The loadout closes the reference: what the frame carries, named the way the
	# simulation names it.
	HudTheme.spacer(_brief_controls, 4.0)
	HudTheme.rule(_brief_controls, HudTheme.HAIRLINE)
	_eyebrow(_brief_controls, "LOADOUT", HudTheme.AMBER)
	for row in LOADOUT:
		var line := HBoxContainer.new()
		line.mouse_filter = Control.MOUSE_FILTER_IGNORE
		line.add_theme_constant_override("separation", 16)
		_brief_controls.add_child(line)
		var key := HudTheme.label(line, row[0], HudTheme.LABEL, HudTheme.AMBER, 0.5, 2)
		key.custom_minimum_size = Vector2(76.0, 0.0)
		key.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
		var name_column := VBoxContainer.new()
		name_column.mouse_filter = Control.MOUSE_FILTER_IGNORE
		name_column.add_theme_constant_override("separation", 0)
		name_column.size_flags_horizontal = Control.SIZE_EXPAND_FILL
		line.add_child(name_column)
		HudTheme.label(name_column, row[1], HudTheme.LABEL, HudTheme.WHITE, 0.3, 1)
		HudTheme.label(name_column, row[2], HudTheme.MICRO, HudTheme.FAINT, 0.2, 1)


func _set_values(targets: Array[Label], values: PackedStringArray) -> void:
	for i in mini(targets.size(), values.size()):
		targets[i].text = values[i]


func _swap(screen: Control) -> void:
	_briefing.visible = screen == _briefing
	_pause.visible = screen == _pause
	_results.visible = screen == _results


func _halt() -> void:
	if _entrance != null and _entrance.is_valid():
		_entrance.kill()
	if _blink != null and _blink.is_valid():
		_blink.kill()


## Fade the screen in and settle its cards down a few pixels. Skipped entirely
## when the system asks for less animation, which is why every animated entry
## point here has an instant path beside it.
func _enter(screen: Control, movers: Array[Control]) -> void:
	_halt()
	if _motion_off:
		screen.modulate.a = 1.0
		for mover in movers:
			_rest(mover)
		return
	screen.modulate.a = 0.0
	_entrance = create_tween()
	_entrance.set_parallel(true)
	_entrance.tween_property(screen, "modulate:a", 1.0, 0.26)
	for mover in movers:
		var rest: Vector2 = mover.get_meta(&"hud_rest", Vector2.ZERO)
		var height := mover.offset_bottom - mover.offset_top
		mover.offset_top = rest.y + 18.0
		mover.offset_bottom = rest.y + 18.0 + height
		_entrance.tween_method(
			_lift.bind(mover, height), rest.y + 18.0, rest.y, 0.36
		).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)


func _lift(value: float, mover: Control, height: float) -> void:
	mover.offset_top = value
	mover.offset_bottom = value + height


func _rest(mover: Control) -> void:
	var rest: Vector2 = mover.get_meta(&"hud_rest", Vector2.ZERO)
	var height := mover.offset_bottom - mover.offset_top
	mover.offset_top = rest.y
	mover.offset_bottom = rest.y + height


func _blink_footer(label: Label) -> void:
	if label == null:
		return
	if _motion_off:
		label.modulate.a = 1.0
		return
	_blink = create_tween().set_loops()
	_blink.tween_property(label, "modulate:a", 0.42, 0.9).set_trans(Tween.TRANS_SINE)
	_blink.tween_property(label, "modulate:a", 1.0, 0.9).set_trans(Tween.TRANS_SINE)
