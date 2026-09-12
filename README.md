# ASHFRAME

Operation ASHEN YARD. A third-person mech action game built on Aurum — a Godot
4.7 host with the game's rules written in Rust.

You pilot a six-metre frame into an industrial yard, break a skirmisher screen,
survive a pair of siege units, and bring down the Patriarch.

---

## Playing it

**You need:** Windows, Rust 1.85 or newer, Godot 4.7, and an Aurum engine
checkout.

```powershell
git clone https://github.com/AG064/ashframe.git ashframe
cd ashframe

# Point the project at your engine checkout:
#   aurum.toml -> [engine] path_hint = "A:/path/to/aurum-studio"

pwsh -File scripts/setup.ps1
pwsh -File scripts/play.ps1
```

`setup.ps1` builds the engine's libraries, installs both add-ons, builds the
game's extension, and imports the Godot project. It is safe to re-run after a
pull and rebuilds only what moved.

If you have the Aurum CLI on your `PATH`, `aurum run` does the same launch with
the engine supervising it.

### Controls

| | |
|---|---|
| `W A S D` | move |
| `Shift` | quick boost |
| `Space` | jump, hold to fly |
| `Ctrl` | assault boost |
| `Left mouse` | autocannon |
| `Right mouse` | missile volley (needs a lock) |
| `V` | arc blade |
| `R` | reload |
| `F` | repair |
| `Q` | lock on |
| `Wheel` | cycle targets |
| `Esc` | pause — `R` from the pause screen restarts |

## What is where

```
crates/ashframe-sim/   the game. No engine, no renderer, no dependencies.
crates/ashframe/       the Godot seam: one node, the meshes, the sound.
godot/                 the project, the scene, and the HUD.
scripts/               setup, play, and the screenshot harness.
```

The split is the point. `ashframe-sim` owns transforms, velocities, timers and
health, and reports what happened through an event enum; it runs headless in its
own test suite in a fraction of a second and knows nothing about Godot.
`ashframe` reads that state and draws it. Nothing in the simulation can reach
the renderer, so the renderer cannot change the game — which is what let the
original Three.js front end be replaced by this one without rewriting the fight.

### The simulation

| | |
|---|---|
| `player.rs` | movement, weapons, energy, damage |
| `enemies.rs` | three archetypes and their steering |
| `projectiles.rs` | bullets, homing missiles, artillery |
| `targeting.rs` | lock-on and the camera assist |
| `collision.rs` | swept spheres, rays, ground probes |
| `arena.rs` | the yard: colliders, and the boxes drawn for them |
| `camera.rs` | third-person follow with an obstruction probe |
| `mission.rs` | Operation ASHEN YARD's phases and statistics |
| `sim.rs` | the whole game, stepped at a fixed 120 Hz |

### The renderer

| | |
|---|---|
| `game.rs` | the node GDScript talks to; one getter per HUD readout |
| `rig.rs` | mech rigs, built from boxes and posed from simulation state |
| `arena_view.rs` | the yard, from the simulation's own description of it |
| `effects.rs` | pooled tracers, impacts, explosions, telegraphs |
| `audio.rs` | voices, pools and the sustained engine |
| `synth.rs` | the offline synthesiser every sound is rendered from |
| `palette.rs` | colours, materials, procedural textures |

Everything GDScript can do to the game node is a getter plus four verbs the
scene layer owns — deploy, restart, pause, unpause. There is no setter for game
state, so a HUD bug cannot become a gameplay bug.

## Development

```powershell
cargo test                     # the simulation, and the renderer's pure parts
cargo clippy --all-targets -- -D warnings
aurum doctor                   # project and toolchain health
aurum build --release          # the shipping extension
```

Both suites and clippy pass on `main`, and a change that breaks one of them is a
change that is not finished.

### The screenshot harness

```powershell
pwsh -File scripts/play.ps1 -Screenshot shots/combat.png -AtFrame 620 -Autoplay -Trace
```

`-Autoplay` drives a canned demonstration through the same control path a person
uses, so a capture shows a fight rather than a mech standing still.
`-DebugCam` holds a fixed three-quarter view of the player for looking at the
rig. `-Trace` prints the simulation state once a second alongside the frame, so
"the camera is wrong" can be told apart from "the mech is somewhere unexpected".

## Building a release

```powershell
aurum build --release
```

Then export through Godot. Export templates are a separate download, from
*Editor → Manage Export Templates*. There is no preset in the repository yet;
the first release adds one, along with the packaging that turns an export into
something somebody can install.

## Known limitations

- **The enemies do not path-find.** They steer, and when the straight line to a
  goal is blocked they slide the goal sideways and try again. A unit can take a
  long way round a large structure and can oscillate against a concave
  arrangement. The honest upgrade is Godot's navmesh, and the module says so.
- **One mission.** The campaign the original's README imagined does not exist.
- **Windows is the only platform anyone has run this on.** The extension
  declares Linux and macOS libraries and nothing has verified them.
- **Godot's headless importer crashes on exit** with the engine's editor
  plug-in loaded. The import itself completes; `setup.ps1` checks what the
  import produced rather than what it exited with.

## Credits

Built with [Aurum](https://github.com/) and
[godot-rust](https://github.com/godot-rust/gdext).

No licence has been chosen. Until one is, this repository is all-rights-reserved
by default, which is worth fixing before anybody is asked to use it.
