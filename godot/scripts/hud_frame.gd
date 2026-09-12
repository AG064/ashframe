## ASHFRAME — the chrome every HUD panel is cut from.
##
## A frame is a panel with a chamfer taken out of two opposite corners, a
## hairline edge, an accent rule along the top and corner ticks where the cut
## corners are not. The chamfer is what stops a translucent rectangle from
## reading as a card floating over the world: two corners cut is a shaped plate,
## four corners rounded is a web widget.
##
## Frames hold a column. Anything a panel wants to say is put in that column, so
## the chrome is drawn once and every panel is spaced by the same rhythm.

class_name HudFrame
extends Control

const CHAMFER := 12.0
const RULE := 84.0

## Colour of the accent rule and of the header text.
var accent: Color = HudTheme.AMBER
## Length of the accent rule along the top edge.
var rule_length: float = RULE
## Draw the chamfer and the corner ticks. The briefing cards turn this off: they
## are read, not glanced at, and want a quieter edge.
var chromium: bool = true

var column: VBoxContainer

var _flash: float = 0.0
var _bottom_anchored: bool = false
var _header: HBoxContainer
var _title: Label
var _meta: Label
var _header_rule: ColorRect


func _init() -> void:
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	set_process(false)

	var margin := MarginContainer.new()
	margin.set_anchors_preset(Control.PRESET_FULL_RECT)
	margin.mouse_filter = Control.MOUSE_FILTER_IGNORE
	margin.add_theme_constant_override("margin_left", int(HudTheme.PAD))
	margin.add_theme_constant_override("margin_right", int(HudTheme.PAD))
	margin.add_theme_constant_override("margin_top", int(HudTheme.PAD) - 2)
	margin.add_theme_constant_override("margin_bottom", int(HudTheme.PAD) - 2)
	add_child(margin)

	column = VBoxContainer.new()
	column.mouse_filter = Control.MOUSE_FILTER_IGNORE
	column.add_theme_constant_override("separation", int(HudTheme.ROW))
	margin.add_child(column)


## Place the frame against one of the viewport's corners. `offset` is measured
## from the anchor and `box` is the panel's size, both in pixels: an anchored
## control is positioned by its offsets, and `Control.position` is the resolved
## rectangle rather than something to set.
func place(preset: int, offset: Vector2, box: Vector2) -> void:
	set_anchors_preset(preset)
	offset_left = offset.x
	offset_top = offset.y
	offset_right = offset.x + box.x
	offset_bottom = offset.y + box.y
	custom_minimum_size = box
	_bottom_anchored = preset in [
		Control.PRESET_BOTTOM_LEFT, Control.PRESET_BOTTOM_RIGHT,
		Control.PRESET_BOTTOM_WIDE, Control.PRESET_CENTER_BOTTOM,
	]


## Grow the frame if the column does not fit the box it was given, keeping the
## edge the frame is anchored to where it is. A panel that holds a variable
## number of rows would otherwise spill its last line past the bottom of the
## screen, which is the one place a readout must never end up.
func fit() -> void:
	var height := offset_bottom - offset_top
	var wanted := column.get_combined_minimum_size().y + HudTheme.PAD * 2.0 - 4.0
	if wanted <= height + 0.5:
		return
	if _bottom_anchored:
		offset_top -= wanted - height
	offset_bottom = offset_top + wanted
	custom_minimum_size = Vector2(maxf(custom_minimum_size.x, size.x), wanted)


## A titled header: the panel's name on the left, a state or a unit on the right,
## and a hairline closing it off from the body. Called once per panel at build
## time; the returned labels are written to at runtime.
func header(title: String, meta: String = "") -> void:
	_header = HBoxContainer.new()
	_header.mouse_filter = Control.MOUSE_FILTER_IGNORE
	_header.add_theme_constant_override("separation", int(HudTheme.U))
	column.add_child(_header)

	_title = HudTheme.label(_header, title, HudTheme.LABEL, accent, 0.45, 2)
	_title.size_flags_horizontal = Control.SIZE_EXPAND_FILL
	_title.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	_meta = HudTheme.label(_header, meta, HudTheme.MICRO, HudTheme.FAINT, 0.3, 1)
	_meta.horizontal_alignment = HORIZONTAL_ALIGNMENT_RIGHT
	_meta.vertical_alignment = VERTICAL_ALIGNMENT_CENTER

	_header_rule = HudTheme.rule(column, HudTheme.HAIRLINE)
	_header_rule.custom_minimum_size = Vector2(0.0, 1.0)


func set_meta_text(text: String) -> void:
	if _meta != null:
		HudTheme.write(_meta, text)


func set_title_colour(colour: Color) -> void:
	if _title != null:
		HudTheme.tint(_title, colour)


## A hit on the frame's subject. The edge goes red for a moment; the panel
## itself does not move, because the panel is where the player is looking when
## they are being shot.
func flash(strength: float = 1.0) -> void:
	_flash = maxf(_flash, clampf(strength, 0.0, 1.0))
	set_process(true)
	queue_redraw()


func _process(delta: float) -> void:
	var busy := false
	if _flash > 0.0:
		_flash = maxf(0.0, _flash - delta * 2.6)
		busy = true
	if _header_rule != null and _flash > 0.0:
		_header_rule.color = HudTheme.HAIRLINE.lerp(HudTheme.RED, _flash * 0.8)
	elif _header_rule != null:
		_header_rule.color = HudTheme.HAIRLINE
	queue_redraw()
	if not busy:
		set_process(false)


func _draw() -> void:
	var w := size.x
	var h := size.y
	var c := CHAMFER if chromium else 0.0

	var plate := PackedVector2Array()
	if chromium:
		plate = PackedVector2Array([
			Vector2(c, 0.0), Vector2(w, 0.0), Vector2(w, h - c),
			Vector2(w - c, h), Vector2(0.0, h), Vector2(0.0, c),
		])
	else:
		plate = PackedVector2Array([
			Vector2(0.0, 0.0), Vector2(w, 0.0), Vector2(w, h), Vector2(0.0, h),
		])

	draw_colored_polygon(plate, HudTheme.INK)
	if _flash > 0.0:
		draw_colored_polygon(plate, Color(HudTheme.RED, 0.10 * _flash))

	var outline := PackedVector2Array(plate)
	outline.append(plate[0])
	draw_polyline(
		outline,
		HudTheme.HAIRLINE.lerp(HudTheme.RED, _flash * 0.9),
		1.0,
		true
	)

	if rule_length > 0.0:
		var lit := HudTheme.RED if _flash > 0.5 else accent
		draw_rect(Rect2(c + 1.0, 0.0, minf(rule_length, maxf(0.0, w - c - 2.0)), 2.0), lit)

	if chromium:
		var tick := HudTheme.HAIRLINE_HOT
		# Ticks on the two corners that are not chamfered, so the plate reads as
		# square where it is not cut.
		draw_line(Vector2(w - 10.0, 0.0), Vector2(w, 0.0), tick, 1.0)
		draw_line(Vector2(w, 0.0), Vector2(w, 10.0), tick, 1.0)
		draw_line(Vector2(0.0, h - 10.0), Vector2(0.0, h), tick, 1.0)
		draw_line(Vector2(0.0, h), Vector2(10.0, h), tick, 1.0)
