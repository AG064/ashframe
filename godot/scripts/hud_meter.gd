## ASHFRAME — one bar, drawn once and used everywhere.
##
## The meter owns a fraction and the motion of that fraction, and nothing else.
## It draws no text: the labels either side of it belong to the panel, because
## the same bar appears as a thin rail under a number and as the boss's health
## strip across the top of the screen.
##
## Two things make it read as an instrument rather than as a rectangle. The
## first is the ghost: damage moves the fill immediately and leaves a brighter
## remnant of what was lost standing for a moment, so a hit is legible even when
## the eye is somewhere else. The second is that the fill eases — up slowly, down
## quickly — because a bar that snaps down reads as a redraw and a bar that eases
## down reads as a loss.

class_name HudMeter
extends Control

## Colour of the fill in its normal state.
var accent: Color = HudTheme.CYAN
## Below this fraction the fill turns red. Negative disables the threshold.
var danger_below: float = -1.0
## Above this fraction the fill turns red. Stability is the one reading where a
## full bar is the bad news, so the meter has to be able to warn upwards.
var danger_above: float = -1.0
## Below this fraction the fill turns amber. Negative disables it.
var warn_below: float = -1.0
## Above this fraction the fill turns amber.
var warn_above: float = -1.0
var danger_colour: Color = HudTheme.RED
var warn_colour: Color = HudTheme.AMBER
## Vertical divisions in the track, drawn as dark ticks. 0 leaves the track bare.
var segments: int = 0
## Leave a remnant standing where the value used to be after a drop.
var ghost: bool = true
## Fractions to mark with a brighter vertical line: an energy floor, a boss's
## phase changes. Empty by default.
var notches := PackedFloat32Array()
## Pulse the fill when it is in the danger band, so a nearly-dead frame draws the
## eye without a separate warning.
var pulse_in_danger: bool = true

var _target: float = 0.0
var _shown: float = 0.0
var _ghost_at: float = 0.0
var _ghost_hold: float = 0.0
var _pulse: float = 0.0


func _init() -> void:
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	set_process(false)


## Set the value the bar is heading for. `instant` is for the first frame of a
## screen, where easing up from empty would look like a bar filling rather than
## like a state.
func set_fraction(fraction: float, instant: bool = false) -> void:
	fraction = clampf(fraction, 0.0, 1.0)
	if instant:
		_target = fraction
		_shown = fraction
		_ghost_at = fraction
		_ghost_hold = 0.0
		set_process(true)
		queue_redraw()
		return
	if is_equal_approx(fraction, _target):
		return
	if fraction < _target and ghost:
		# Hold the ghost at the highest value on screen, so a second hit taken
		# while the first is still draining does not shorten the trail.
		_ghost_at = maxf(_ghost_at, _shown)
		_ghost_hold = 0.45
	_target = fraction
	set_process(true)


func _process(delta: float) -> void:
	var busy := false
	# Down fast, up slow. Losing is an event; regaining is a process.
	if not is_equal_approx(_shown, _target):
		_shown = _approach(_shown, _target, 8.5 if _target < _shown else 3.2, delta)
		busy = true
	if _ghost_hold > 0.0:
		_ghost_hold -= delta
		busy = true
	elif not is_equal_approx(_ghost_at, _target):
		_ghost_at = _approach(_ghost_at, _target, 1.7, delta)
		busy = true
	if _in_danger():
		_pulse += delta * 3.6
		busy = true
	queue_redraw()
	if not busy:
		_shown = _target
		_ghost_at = _target
		set_process(false)


func _draw() -> void:
	var height := size.y
	var width := size.x
	draw_rect(Rect2(0.0, 0.0, width, height), HudTheme.WELL)
	draw_line(Vector2(0.0, 0.5), Vector2(width, 0.5), HudTheme.HAIRLINE, 1.0)
	draw_line(Vector2(0.0, height - 0.5), Vector2(width, height - 0.5), HudTheme.HAIRLINE, 1.0)

	var fill := _fill_colour()
	if ghost and _ghost_at > _shown + 0.004:
		draw_rect(
			Rect2(0.0, 0.0, width * _ghost_at, height),
			Color(fill.lerp(Color.WHITE, 0.35), 0.26)
		)

	var lit := width * _shown
	if lit > 0.5:
		var strength := 1.0
		if _in_danger() and pulse_in_danger:
			strength = 0.72 + 0.28 * sin(_pulse)
		draw_rect(Rect2(0.0, 0.0, lit, height), Color(fill, strength))
		# The leading edge is what the eye tracks, so it is the brightest pixel
		# on the bar.
		draw_rect(
			Rect2(maxf(0.0, lit - 2.0), 0.0, minf(2.0, lit), height),
			Color(fill.lerp(Color.WHITE, 0.6), strength)
		)

	if segments > 1:
		var tick := Color(0.02, 0.026, 0.033, 0.34)
		for i in range(1, segments):
			var x := floorf(width * float(i) / float(segments)) + 0.5
			draw_line(Vector2(x, 0.0), Vector2(x, height), tick, 1.0)

	for notch in notches:
		if notch <= 0.0 or notch >= 1.0:
			continue
		var x := width * notch
		draw_line(Vector2(x, 0.0), Vector2(x, height), Color(0.98, 0.99, 1.0, 0.5), 1.0)


func _fill_colour() -> Color:
	if danger_below >= 0.0 and _target <= danger_below:
		return danger_colour
	if danger_above >= 0.0 and _target >= danger_above:
		return danger_colour
	if warn_below >= 0.0 and _target <= warn_below:
		return warn_colour
	if warn_above >= 0.0 and _target >= warn_above:
		return warn_colour
	return accent


func _in_danger() -> bool:
	if danger_below >= 0.0 and _target <= danger_below:
		return true
	return danger_above >= 0.0 and _target >= danger_above


static func _approach(current: float, target: float, rate: float, delta: float) -> float:
	return lerpf(current, target, clampf(1.0 - exp(-rate * delta), 0.0, 1.0))
