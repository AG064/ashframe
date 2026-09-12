## ASHFRAME — the gunsight.
##
## The reticle is drawn rather than assembled. Everything in it is an angle, a
## radius or a line, and the same figure has to open with speed, close on a
## target, spin while it tracks, splay when the frame staggers and carry a
## readout — none of which survives being built out of rotated rectangles.
##
## It is told the truth once a frame and draws what follows from it. The only
## state it keeps is the motion: the lock point eases toward the target instead
## of teleporting, the bloom from firing decays, the brackets fly in when a lock
## is new. That is the difference between a crosshair and something that looks
## like it is aiming.

class_name HudReticle
extends Control

## Under every light line in the sight: one pixel down, one pixel wider.
const SHADOW := Color(0.0, 0.0, 0.0, 0.45)

# ── what the HUD tells it, once a frame ─────────────────────────────────────

var speed: float = 0.0
var altitude: float = 0.0
var grounded: bool = true
var boosting: bool = false
var staggered: bool = false
var reloading: bool = false
var reload_progress: float = 0.0
var has_lock: bool = false
var lock_point: Vector2 = Vector2.ZERO
var lock_name: String = ""
var lock_distance: float = 0.0
var lock_health: float = 0.0
## World positions of live hostiles, trusted only while the count on the rigs
## agrees with the count in the simulation.
var contacts := PackedVector3Array()
var contacts_trusted: bool = false
## Bearing of the nearest contact, in screen radians, and how hard the frame was
## hit: used to point at whatever is shooting.
var threat_bearing: float = 0.0
var threat: float = 0.0
var reduced_motion: bool = false
var live: bool = true

# ── motion state ────────────────────────────────────────────────────────────

var _bloom: float = 0.0
var _lock_display: Vector2 = Vector2.ZERO
var _lock_age: float = 99.0
var _lock_last_name: String = ""
var _lock_last_point: Vector2 = Vector2.ZERO
var _spin: float = 0.0
var _time: float = 0.0
var _threat_bearing_display: float = 0.0
var _speed_display: float = 0.0

var _caption_font: Font
var _num_font: Font
var _caption: String = ""
var _caption_width: float = 0.0
var _range_text: String = ""
var _range_width: float = 0.0
var _range_metres: int = -1
var _speed_text: String = ""
var _last_full_speed: int = -1
var _last_full_alt: int = -1


func _init() -> void:
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	set_anchors_preset(Control.PRESET_FULL_RECT)
	_caption_font = HudTheme.font(HudTheme.LABEL, 0.35, 1)
	_num_font = HudTheme.font(HudTheme.MICRO, 0.3, 1)


## Called when the frame fires. The sight blooms and closes again; that bloom is
## the only feedback the player gets at the centre of the screen, where they are
## already looking.
func recoil(strength: float = 1.0) -> void:
	_bloom = minf(1.6, _bloom + strength)


func _process(delta: float) -> void:
	_time += delta
	_spin = fmod(_spin + delta * (2.1 if has_lock else 0.55), TAU)
	_bloom = maxf(0.0, _bloom - delta * 3.4)
	_speed_display = lerpf(_speed_display, speed, clampf(delta * 6.0, 0.0, 1.0))
	threat = maxf(0.0, threat - delta * 1.5)
	var threat_target := threat_bearing
	if not reduced_motion:
		# Take the short way round rather than unwinding through a full turn.
		var diff := wrapf(threat_target - _threat_bearing_display, -PI, PI)
		_threat_bearing_display += diff * clampf(delta * 8.0, 0.0, 1.0)
	else:
		_threat_bearing_display = threat_target
	_update_lock(delta)
	queue_redraw()


func _update_lock(delta: float) -> void:
	if has_lock:
		if lock_name != _lock_last_name or lock_point.distance_to(_lock_last_point) > 70.0:
			_lock_age = 0.0
		_lock_last_name = lock_name
		_lock_last_point = lock_point
		_lock_age = minf(_lock_age + delta, 99.0)
		if _lock_age < 0.01:
			_lock_display = lock_point
		else:
			_lock_display = _lock_display.lerp(lock_point, clampf(delta * 14.0, 0.0, 1.0))
	else:
		_lock_age = 99.0


func _draw() -> void:
	if not live:
		return
	var centre := size * 0.5
	if not reduced_motion and staggered:
		# A frame that has lost its footing cannot hold a sight steady. Two
		# frequencies so the shake does not look like a single sine wave.
		centre += Vector2(sin(_time * 37.0), cos(_time * 29.0)) * 3.2

	var opened := clampf(_speed_display / 48.0, 0.0, 1.35)
	var spread := 13.0 + opened * 21.0 + _bloom * 7.0
	var base := HudTheme.RED if staggered else HudTheme.WHITE

	_draw_ring(centre, spread)
	_draw_cross(centre, spread, base)

	if reloading:
		_draw_reload_arc(centre, spread)

	if threat > 0.01:
		_draw_threat(centre)

	if contacts_trusted:
		_draw_contacts(centre)

	if has_lock:
		_draw_lock(centre)

	_draw_frame_readout(centre)


# ── the sight itself ────────────────────────────────────────────────────────

## Every light line in the sight is drawn twice: once as a dark copy a pixel down,
## once in its own colour. The yard is a bright place — sand, concrete, a pale
## sky — and a white crosshair on it is a crosshair nobody can see. The shadow is
## what a real optic's etched reticle gets from being etched.
func _sight_line(from: Vector2, to: Vector2, colour: Color, width: float) -> void:
	draw_line(from + Vector2(0.0, 1.0), to + Vector2(0.0, 1.0), SHADOW, width + 1.0)
	draw_line(from, to, colour, width)


func _sight_arc(
	centre: Vector2, radius: float, start: float, end: float, points: int,
	colour: Color, width: float
) -> void:
	draw_arc(centre, radius, start, end, points, SHADOW, width + 1.0, true)
	draw_arc(centre, radius, start, end, points, colour, width, true)


func _draw_ring(centre: Vector2, spread: float) -> void:
	var radius := 44.0 + spread * 0.30
	var faint := HudTheme.CYAN if has_lock else HudTheme.WHITE
	var alpha := 0.52 if has_lock else 0.34
	# Four arcs with the axes left open: the gaps are what tell the eye where
	# the sight is pointing when the sight is over a busy background.
	for quarter in 4:
		var start := TAU * float(quarter) / 4.0 + 0.21
		_sight_arc(centre, radius, start, start + 0.86, 30, Color(faint, alpha), 1.0)
	# Scale ticks on the diagonals, and a shorter pair between them: a ring with
	# eight graduations reads as an instrument, a bare circle reads as a cursor.
	for diagonal in 8:
		var angle := TAU * float(diagonal) / 8.0 + TAU / 16.0
		var direction := Vector2(cos(angle), sin(angle))
		var length := 5.0 if diagonal % 2 == 0 else 3.0
		_sight_line(
			centre + direction * (radius - length),
			centre + direction * (radius + length),
			Color(faint, alpha * 1.25), 1.0
		)


func _draw_cross(centre: Vector2, spread: float, base: Color) -> void:
	var heavy := Color(base, 0.94 if not staggered else 0.96)
	var light := Color(base, 0.58)
	# A gap cross at the centre, then the spread ticks out where the shot could
	# actually land. The gap is what makes the middle of the sight readable
	# instead of a knot.
	for axis in 4:
		var angle := TAU * float(axis) / 4.0
		var direction := Vector2(cos(angle), sin(angle))
		_sight_line(centre + direction * 4.0, centre + direction * 11.0, heavy, 2.0)
		var inward := centre + direction * spread
		_sight_line(inward, centre + direction * (spread + 9.0), heavy, 2.0)
		_sight_line(
			centre + direction * (spread + 9.0),
			centre + direction * (spread + 15.0), light, 1.0
		)
	# Four diagonal ticks, sitting between the spread ticks: they close the sight
	# in without adding another line to read.
	for diagonal in 4:
		var angle := TAU * float(diagonal) / 4.0 + TAU / 8.0
		var direction := Vector2(cos(angle), sin(angle))
		_sight_line(
			centre + direction * (spread * 0.82),
			centre + direction * (spread * 0.82 + 5.0), Color(base, 0.40), 1.0
		)

	var core := HudTheme.AMBER if boosting else base
	draw_circle(centre, 3.2, SHADOW)
	draw_circle(centre, 1.9, Color(core, 0.98))
	if boosting and not reduced_motion:
		draw_circle(centre, 4.0 + sin(_time * 12.0) * 1.2, Color(HudTheme.AMBER, 0.30))


func _draw_reload_arc(centre: Vector2, spread: float) -> void:
	var radius := 44.0 + spread * 0.30 + 9.0
	draw_arc(
		centre, radius, -PI * 0.5, -PI * 0.5 + TAU * clampf(reload_progress, 0.0, 1.0),
		40, Color(HudTheme.AMBER, 0.85), 2.0, true
	)


func _draw_threat(centre: Vector2) -> void:
	var radius := 44.0 + 13.0 * 0.30 + 30.0
	var strength := clampf(threat, 0.0, 1.0)
	var half := 0.30 + 0.10 * strength
	var arc := Color(HudTheme.RED, 0.80 * strength)
	draw_arc(
		centre, radius, _threat_bearing_display - half, _threat_bearing_display + half,
		18, SHADOW, 4.0, true
	)
	draw_arc(
		centre, radius, _threat_bearing_display - half, _threat_bearing_display + half,
		18, arc, 3.0, true
	)
	var direction := Vector2(cos(_threat_bearing_display), sin(_threat_bearing_display))
	var tip := centre + direction * (radius + 9.0)
	var side := Vector2(-direction.y, direction.x) * 5.0
	draw_colored_polygon(
		PackedVector2Array([
			tip,
			centre + direction * (radius + 1.0) + side,
			centre + direction * (radius + 1.0) - side,
		]),
		Color(HudTheme.RED, 0.85 * strength)
	)


# ── everything that is not the player ───────────────────────────────────────

## Off-screen contacts get a chevron on a ring around the sight; on-screen ones
## get four faint corner ticks and nothing else, because a box on every hostile
## is a box on top of the only thing the player is shooting at.
func _draw_contacts(centre: Vector2) -> void:
	var camera := get_viewport().get_camera_3d()
	if camera == null:
		return
	var ring := minf(size.x, size.y) * 0.5 - minf(size.x, size.y) * 0.18
	for i in contacts.size():
		var world: Vector3 = contacts[i]
		if camera.is_position_behind(world):
			continue
		var point := camera.unproject_position(world)
		var offset := point - centre
		var distance := offset.length()
		if distance > ring:
			_draw_contact_chevron(centre, offset.normalized(), ring)
		elif distance > 30.0:
			_draw_contact_pip(point)


func _draw_contact_pip(point: Vector2) -> void:
	var colour := Color(HudTheme.RED, 0.62)
	var arm := 6.0
	draw_line(point + Vector2(-arm, -arm), point + Vector2(-arm + 3.0, -arm), colour, 1.0)
	draw_line(point + Vector2(-arm, -arm), point + Vector2(-arm, -arm + 3.0), colour, 1.0)
	draw_line(point + Vector2(arm, -arm), point + Vector2(arm - 3.0, -arm), colour, 1.0)
	draw_line(point + Vector2(arm, -arm), point + Vector2(arm, -arm + 3.0), colour, 1.0)
	draw_line(point + Vector2(-arm, arm), point + Vector2(-arm + 3.0, arm), colour, 1.0)
	draw_line(point + Vector2(-arm, arm), point + Vector2(-arm, arm - 3.0), colour, 1.0)
	draw_line(point + Vector2(arm, arm), point + Vector2(arm - 3.0, arm), colour, 1.0)
	draw_line(point + Vector2(arm, arm), point + Vector2(arm, arm - 3.0), colour, 1.0)


func _draw_contact_chevron(centre: Vector2, direction: Vector2, ring: float) -> void:
	var side := Vector2(-direction.y, direction.x)
	var tip := centre + direction * (ring + 7.0)
	var back := centre + direction * (ring - 3.0)
	var colour := Color(HudTheme.RED, 0.55)
	# Four lines rather than a filled polygon: a polygon is an array, and an
	# array per contact per frame is exactly the kind of allocation a HUD that
	# runs continuously should not be making.
	draw_line(back + side * 6.0, tip, colour, 2.0)
	draw_line(back - side * 6.0, tip, colour, 2.0)
	draw_line(back + side * 6.0, back + side * 2.4, colour, 2.0)
	draw_line(back - side * 6.0, back - side * 2.4, colour, 2.0)


# ── the lock ────────────────────────────────────────────────────────────────

func _draw_lock(centre: Vector2) -> void:
	var radius := clampf(1600.0 / maxf(lock_distance, 10.0), 26.0, 110.0)
	# New locks fly in from wide and settle, so a lock changing target is an
	# event rather than a bracket appearing somewhere else.
	var settle := clampf(_lock_age / 0.22, 0.0, 1.0)
	var eased := 1.0 - pow(1.0 - settle, 3.0)
	var ring := lerpf(radius * 2.1, radius, eased if not reduced_motion else 1.0)
	var alpha := 0.35 + 0.65 * (eased if not reduced_motion else 1.0)
	var corner := HudTheme.CYAN
	var arm := maxf(8.0, ring * 0.42)
	# A lock that has drifted off the edge of the frame is still a lock, and
	# losing the readout at the moment the target leaves the frame is the worst
	# possible time to lose it, so the bracket is held inside the picture.
	var inset := Vector2(130.0, 140.0)
	var at := Vector2(
		clampf(_lock_display.x, inset.x, maxf(inset.x, size.x - inset.x)),
		clampf(_lock_display.y, inset.y, maxf(inset.y, size.y - inset.y))
	)

	for corner_index in 4:
		var sx := -1.0 if corner_index in [0, 2] else 1.0
		var sy := -1.0 if corner_index in [0, 1] else 1.0
		var origin := at + Vector2(sx * ring, sy * ring)
		draw_line(origin, origin + Vector2(-sx * arm, 0.0), Color(corner, alpha), 2.0)
		draw_line(origin, origin + Vector2(0.0, -sy * arm), Color(corner, alpha), 2.0)

	# Two arcs chasing each other around the bracket: the lock is being held,
	# not merely drawn.
	if not reduced_motion:
		var orbit := ring * 1.32
		draw_arc(at, orbit, _spin, _spin + 0.7, 16, Color(corner, 0.45 * alpha), 1.0, true)
		draw_arc(
			at, orbit, _spin + PI, _spin + PI + 0.7, 16,
			Color(corner, 0.45 * alpha), 1.0, true
		)

	# A stalk from the bracket down to the readout, so the name belongs to the
	# target instead of floating near it.
	var readout_y := at.y + ring + 22.0
	draw_line(
		at + Vector2(0.0, ring + arm * 0.2), Vector2(at.x, readout_y - 12.0),
		Color(corner, 0.35 * alpha), 1.0
	)

	if _caption != lock_name:
		_caption = lock_name
		_caption_width = _caption_font.get_string_size(
			_caption, HORIZONTAL_ALIGNMENT_LEFT, -1, HudTheme.LABEL
		).x
	# The range is rebuilt on the metre it changes on, not on the frame: a
	# formatted string per frame is a garbage collection per frame, and this
	# drawing code runs the whole time the player is in the yard.
	var range_metres := roundi(lock_distance)
	if range_metres != _range_metres:
		_range_metres = range_metres
		_range_text = "%d M" % range_metres
		_range_width = _num_font.get_string_size(
			_range_text, HORIZONTAL_ALIGNMENT_LEFT, -1, HudTheme.MICRO
		).x

	draw_string_outline(
		_caption_font, Vector2(at.x - _caption_width * 0.5, readout_y),
		_caption, HORIZONTAL_ALIGNMENT_LEFT, -1, HudTheme.LABEL, 4, SHADOW
	)
	draw_string(
		_caption_font, Vector2(at.x - _caption_width * 0.5, readout_y),
		_caption, HORIZONTAL_ALIGNMENT_LEFT, -1, HudTheme.LABEL,
		Color(HudTheme.WHITE, alpha)
	)
	draw_string_outline(
		_num_font, Vector2(at.x - _range_width * 0.5, readout_y + 14.0),
		_range_text, HORIZONTAL_ALIGNMENT_LEFT, -1, HudTheme.MICRO, 4, SHADOW
	)
	draw_string(
		_num_font, Vector2(at.x - _range_width * 0.5, readout_y + 14.0),
		_range_text, HORIZONTAL_ALIGNMENT_LEFT, -1, HudTheme.MICRO,
		Color(HudTheme.CYAN, alpha)
	)

	# The target's own health, under its name: the one place a bar belongs that
	# is not the player's.
	var bar_width := maxf(68.0, ring * 1.6)
	var bar := Rect2(at.x - bar_width * 0.5, readout_y + 20.0, bar_width, 4.0)
	draw_rect(Rect2(bar.position - Vector2(0.0, 1.0), bar.size + Vector2(0.0, 2.0)), SHADOW)
	draw_rect(bar, Color(0.0, 0.0, 0.0, 0.5))
	draw_rect(
		Rect2(bar.position, Vector2(bar_width * clampf(lock_health, 0.0, 1.0), 4.0)),
		Color(HudTheme.RED, 0.95 * alpha)
	)


# ── the frame's own numbers ─────────────────────────────────────────────────

func _draw_frame_readout(centre: Vector2) -> void:
	var speed_metres := roundi(_speed_display)
	var altitude_metres := roundi(maxf(0.0, altitude))
	if speed_metres != _last_full_speed or altitude_metres != _last_full_alt:
		_last_full_speed = speed_metres
		_last_full_alt = altitude_metres
		_speed_text = "SPD %03d    ALT %03d    %s" % [
			speed_metres, altitude_metres, "GND" if grounded else "AIR",
		]
	var y := centre.y + 78.0
	# Outlined, because this line crosses whatever the camera happens to be
	# pointing at and the yard is mostly bright.
	draw_string_outline(
		_num_font, Vector2(centre.x - 150.0, y), _speed_text,
		HORIZONTAL_ALIGNMENT_CENTER, 300.0, HudTheme.MICRO, 3,
		Color(0.0, 0.0, 0.0, 0.65)
	)
	draw_string(
		_num_font, Vector2(centre.x - 150.0, y), _speed_text,
		HORIZONTAL_ALIGNMENT_CENTER, 300.0, HudTheme.MICRO,
		Color(HudTheme.DIM, 0.95)
	)
