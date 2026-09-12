## ASHFRAME — the design language the HUD is drawn in.
##
## Everything on screen comes from here: the palette, five type sizes, the
## spacing unit and the fonts the interface is allowed to use. A colour or a size
## that is not named in this file does not belong on screen, which is what stops
## an interface assembled over several passes from drifting into a pile of
## slightly different greys.
##
## The fonts are built, not imported. The project ships no typeface, and the
## engine's fallback has no bold or tracked cut — so `font()` hands back cached
## `FontVariation`s. Emboldening and letter spacing are enough to get a hierarchy
## out of one face, which is cheaper than adding a font file and its licence to
## the repository for a HUD that is mostly capitals and digits.
##
## The palette is deliberately small: cold steel to draw on, instrument amber for
## the frame's own systems, cyan for what the machine is being told, red for what
## is hurting it. Two greys carry text, and they do it at three weights rather
## than three colours.

class_name HudTheme
extends RefCounted

# ── palette ─────────────────────────────────────────────────────────────────

## Panel fill. Translucent on purpose: the yard should read through the chrome,
## because a HUD that blacks out the world is a menu.
const INK := Color(0.031, 0.043, 0.055, 0.80)
## Overlay card fill, nearly solid so briefing text never fights the terrain.
const INK_DEEP := Color(0.014, 0.021, 0.028, 0.96)
## A well the bars sit in.
const WELL := Color(0.0, 0.0, 0.0, 0.42)
const HAIRLINE := Color(0.66, 0.72, 0.78, 0.26)
const HAIRLINE_HOT := Color(0.82, 0.87, 0.92, 0.48)

const AMBER := Color(0.96, 0.68, 0.26)
const CYAN := Color(0.38, 0.79, 0.92)
const RED := Color(0.91, 0.29, 0.21)
const WHITE := Color(0.95, 0.97, 0.98)
const DIM := Color(0.72, 0.77, 0.82)
const FAINT := Color(0.47, 0.53, 0.59)

# ── type scale ──────────────────────────────────────────────────────────────
#
# Five sizes and no more. They are far enough apart that any two of them read as
# a deliberate pairing rather than as a mistake: a screen title, a number, a
# sentence, a label, a footnote.

const HUGE := 44
const BIG := 30
const BODY := 16
const LABEL := 12
const MICRO := 10

# ── spacing rhythm ──────────────────────────────────────────────────────────
#
# Everything is a multiple of six. Panels sit one margin from the screen edge
# and one gutter apart, and every gap inside a panel is U or a small multiple.

const U := 6.0
const MARGIN := 26.0
const PAD := 14.0
const ROW := 8.0

# ── type ────────────────────────────────────────────────────────────────────

static var _fonts: Dictionary = {}


## One face, three weights. `bold` is a FontVariation embolden factor: 0.0 is
## the plain cut, 0.35 reads as semibold at HUD sizes, 0.7 as bold. `tracking`
## is extra pixels between glyphs, which is what makes a row of capitals look
## engineered instead of merely shouted.
static func font(size: int, bold: float = 0.0, tracking: int = 0) -> Font:
	var key := size * 100000 + int(bold * 100.0) * 100 + tracking + 50
	var cached: Variant = _fonts.get(key)
	if cached != null:
		return cached
	var face := FontVariation.new()
	face.base_font = ThemeDB.fallback_font
	face.variation_embolden = bold
	face.spacing_glyph = tracking
	_fonts[key] = face
	return face


## A label with the theme already applied. Everything the HUD writes text into
## is made here, so no call site has to remember the font overrides.
static func label(
	parent: Node,
	text: String,
	size: int,
	colour: Color,
	bold: float = 0.0,
	tracking: int = 0
) -> Label:
	var node := Label.new()
	node.text = text
	node.mouse_filter = Control.MOUSE_FILTER_IGNORE
	node.add_theme_font_override("font", font(size, bold, tracking))
	node.add_theme_font_size_override("font_size", size)
	node.add_theme_color_override("font_color", colour)
	node.set_meta(&"hud_tint", colour)
	parent.add_child(node)
	return node


## Recolour without touching the theme if the colour has not actually changed.
## Per-frame `add_theme_color_override` calls dirty the label's theme cache on
## every frame for no reason, and half of this HUD's labels change one state at
## a time.
static func tint(node: Label, colour: Color) -> void:
	var current: Variant = node.get_meta(&"hud_tint", colour)
	if current == colour:
		return
	node.set_meta(&"hud_tint", colour)
	node.add_theme_color_override("font_color", colour)


## Rewrite text only when it differs. Same reasoning as `tint`, and it also
## keeps string formatting out of the frame path.
static func write(node: Label, text: String) -> void:
	if node.text != text:
		node.text = text


## A hard shadow under text that is drawn over the world rather than over a
## panel. The floats and the stagger banner move across whatever the camera
## happens to be pointing at, and the yard is mostly bright sand.
static func shadow(node: Label, offset: int = 1) -> void:
	node.add_theme_color_override("font_shadow_color", Color(0.0, 0.0, 0.0, 0.75))
	node.add_theme_constant_override("shadow_offset_x", offset)
	node.add_theme_constant_override("shadow_offset_y", offset)
	node.add_theme_constant_override("shadow_outline_size", 2)


static func rule(parent: Node, colour: Color, height: float = 1.0) -> ColorRect:
	var line := ColorRect.new()
	line.color = colour
	line.custom_minimum_size = Vector2(0.0, height)
	line.mouse_filter = Control.MOUSE_FILTER_IGNORE
	parent.add_child(line)
	return line


static func spacer(parent: Node, height: float) -> Control:
	var gap := Control.new()
	gap.custom_minimum_size = Vector2(0.0, height)
	gap.mouse_filter = Control.MOUSE_FILTER_IGNORE
	parent.add_child(gap)
	return gap


## A flat panel, for the few places a StyleBox is the right tool: the overlay
## cards, which are containers of containers and do not want chrome drawn under
## their children.
static func card(bg: Color, edge: Color, edge_width: int = 1, radius: int = 6) -> StyleBoxFlat:
	var style := StyleBoxFlat.new()
	style.bg_color = bg
	style.border_color = edge
	style.set_border_width_all(edge_width)
	style.set_corner_radius_all(radius)
	style.content_margin_left = PAD
	style.content_margin_right = PAD
	style.content_margin_top = PAD
	style.content_margin_bottom = PAD
	return style


# ── numbers ─────────────────────────────────────────────────────────────────

## Thousands separated by a thin space rather than a comma, because a comma at
## HUD sizes reads as a full stop.
static func thousands(value: int) -> String:
	var digits := str(absi(value))
	var out := ""
	var count := 0
	for i in range(digits.length() - 1, -1, -1):
		out = digits[i] + out
		count += 1
		if count % 3 == 0 and i > 0:
			out = " " + out
	return ("-" if value < 0 else "") + out


## A clock that never shows more than two digits of minutes.
static func clock(seconds: float) -> String:
	var total := maxi(0, int(seconds))
	return "%02d:%02d" % [total / 60, total % 60]
