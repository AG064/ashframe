//! Mech visuals.
//!
//! Every mech in the game is built from boxes at load time and posed from
//! simulation state every frame. There is no model file and no animation clip:
//! the pose is arithmetic over the same numbers the simulation already
//! publishes, which is what keeps the renderer honest — it cannot show a mech
//! doing something the simulation is not doing, because it has nothing else to
//! read.
//!
//! The rig is deliberately articulated rather than a single frozen mesh. Legs
//! that swing against each other, a torso that lags the turn, a lean into
//! acceleration: those are most of what separates a mech that reads as a
//! machine walking from a box sliding across a floor.
//!
//! ## Why the shapes are what they are
//!
//! A mech made of one box per limb reads as furniture. What makes it read as a
//! machine is that every mass has a reason to be where it is: the chest is the
//! thickest part because the cockpit and the reactor are there, the shoulders
//! stand off it on visible joints because arms that reach past the body need
//! the clearance, the legs are wider at the hip than at the ankle because that
//! is where the load is. None of that is decoration; it is the same reason the
//! silhouette of a real vehicle is legible at a distance.

use godot::classes::{Node3D, StandardMaterial3D};
use godot::prelude::*;

use ashframe_sim::types::{EnemyKind, Vec3};

use crate::palette::{box_node, glow, material, mech_color, to_godot, MechSpec};

/// Everything the renderer needs to place a mech this frame.
///
/// All of it is read from the simulation. Nothing in this struct is decided by
/// the renderer, and nothing here is written back.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MechPose {
    /// Feet position.
    pub at: Vec3,
    /// Facing of the whole machine.
    pub yaw: f32,
    /// Torso yaw, which trails the body.
    pub torso_yaw: f32,
    /// Lean into acceleration, in radians.
    pub lean_x: f32,
    pub lean_z: f32,
    /// Walk cycle phase, advanced by distance travelled rather than by time.
    pub gait: f32,
    pub airborne: bool,
    /// 0..1, how hard the thrusters are burning.
    pub thrust: f32,
    /// 0..1, how doubled over from a stagger.
    pub stagger: f32,
    /// 0..1 visual recoil on the gun arm.
    pub recoil: f32,
    /// 0..1 blade extension.
    pub blade: f32,
    /// 0..1 flash, from taking a hit.
    pub hit_flash: f32,
}

/// Which mech is being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Player,
    Hostile(EnemyKind),
}

/// One articulated mech.
pub struct MechRig {
    pub root: Gd<Node3D>,
    torso: Gd<Node3D>,
    head: Gd<Node3D>,
    left_leg: Gd<Node3D>,
    right_leg: Gd<Node3D>,
    left_knee: Gd<Node3D>,
    right_knee: Gd<Node3D>,
    left_foot: Gd<Node3D>,
    right_foot: Gd<Node3D>,
    gun: Gd<Node3D>,
    blade_arm: Gd<Node3D>,
    blade: Gd<Node3D>,
    thruster_left: Gd<Node3D>,
    thruster_right: Gd<Node3D>,
    shell: Gd<StandardMaterial3D>,
    trim: Gd<StandardMaterial3D>,
    spec: MechSpec,
}

impl MechRig {
    /// Build a mech and return it with its root already parented.
    pub fn new(parent: &mut Gd<Node3D>, frame: Frame) -> Self {
        let spec = MechSpec::for_frame(frame);
        let accent = match frame {
            Frame::Player => mech_color::PLAYER_GLASS,
            Frame::Hostile(EnemyKind::Skirmisher) => mech_color::SKIRMISHER_TRIM,
            Frame::Hostile(EnemyKind::Artillery) => mech_color::ARTILLERY_TRIM,
            Frame::Hostile(EnemyKind::Boss) => mech_color::BOSS_TRIM,
        };

        let shell = material(spec.shell_color, 0.55, 0.45);
        let trim = material(spec.trim_color, 0.75, 0.30);
        let dark = material(mech_color::DARK, 0.25, 0.8);
        // Emissive parts are lamps, not paint: enough to read as powered at
        // range without turning the machine into a lantern.
        let visor = glow(accent, 0.85);

        let mut root = Node3D::new_alloc();

        // ── legs ────────────────────────────────────────────────────────────
        // Each leg is a hip pivot carrying a thigh, a knee pivot, an ankle and
        // a foot. Three joints is the least that reads as a walk rather than as
        // a pendulum on a stick.
        let leg_offset = spec.hip_width * 0.5;
        let (mut left_leg, left_knee, left_foot) = build_leg(&dark, &shell, &trim, spec, &mut root);
        let (mut right_leg, right_knee, right_foot) =
            build_leg(&dark, &shell, &trim, spec, &mut root);
        left_leg.set_position(Vector3::new(leg_offset, 0.0, 0.0));
        right_leg.set_position(Vector3::new(-leg_offset, 0.0, 0.0));

        // ── torso ───────────────────────────────────────────────────────────
        let mut torso = Node3D::new_alloc();
        torso.set_position(Vector3::new(0.0, spec.hip_height, 0.0));
        root.add_child(&torso);

        let mut pelvis = box_node(
            Vector3::new(
                spec.hip_width * 1.35,
                spec.waist_height,
                spec.body_depth * 0.9,
            ),
            &dark,
        );
        pelvis.set_position(Vector3::new(0.0, spec.waist_height * 0.5, 0.0));
        torso.add_child(&pelvis);

        let mut waist = box_node(
            Vector3::new(
                spec.body_width * 0.55,
                spec.waist_height * 1.1,
                spec.body_depth * 0.7,
            ),
            &trim,
        );
        waist.set_position(Vector3::new(0.0, spec.waist_height * 1.55, 0.0));
        torso.add_child(&waist);

        // The chest is the cockpit and the reactor, so it is the thickest mass
        // on the machine, and it sits forward of the spine rather than centred
        // on it.
        let chest_y = spec.waist_height * 1.1 + spec.chest_height * 0.5;
        let mut chest = box_node(
            Vector3::new(spec.body_width, spec.chest_height, spec.body_depth),
            &shell,
        );
        chest.set_position(Vector3::new(0.0, chest_y, spec.body_depth * 0.08));
        torso.add_child(&chest);

        let mut spine = box_node(
            Vector3::new(
                spec.body_width * 0.42,
                spec.chest_height * 0.85,
                spec.body_depth * 0.35,
            ),
            &dark,
        );
        spine.set_position(Vector3::new(0.0, chest_y, -spec.body_depth * 0.52));
        torso.add_child(&spine);

        let mut collar = box_node(
            Vector3::new(
                spec.body_width * 0.7,
                spec.chest_height * 0.22,
                spec.body_depth * 0.9,
            ),
            &trim,
        );
        collar.set_position(Vector3::new(
            0.0,
            chest_y + spec.chest_height * 0.6,
            spec.body_depth * 0.05,
        ));
        torso.add_child(&collar);

        // The cockpit window is on the chest rather than the face: a pilot sits
        // in the torso, and a glowing face on a small head reads as a robot
        // rather than as a machine somebody is inside.
        let mut cockpit = box_node(
            Vector3::new(
                spec.body_width * 0.44,
                spec.chest_height * 0.15,
                spec.body_depth * 0.05,
            ),
            &visor,
        );
        cockpit.set_position(Vector3::new(
            0.0,
            chest_y + spec.chest_height * 0.22,
            spec.body_depth * 0.58,
        ));
        torso.add_child(&cockpit);

        // A plate below the window, so the front of the chest is a face with a
        // window in it rather than a black box with a light on it.
        let mut breastplate = box_node(
            Vector3::new(
                spec.body_width * 0.88,
                spec.chest_height * 0.52,
                spec.body_depth * 0.06,
            ),
            &shell,
        );
        breastplate.set_position(Vector3::new(
            0.0,
            chest_y - spec.chest_height * 0.20,
            spec.body_depth * 0.56,
        ));
        torso.add_child(&breastplate);

        // Two stripes, not a panel. The trim is what tells one frame from
        // another at a glance, and it only does that while it stays rare.
        for side in [1.0f32, -1.0] {
            let mut stripe = box_node(
                Vector3::new(
                    spec.body_width * 0.10,
                    spec.chest_height * 0.50,
                    spec.body_depth * 0.05,
                ),
                &trim,
            );
            stripe.set_position(Vector3::new(
                side * spec.body_width * 0.31,
                chest_y - spec.chest_height * 0.20,
                spec.body_depth * 0.60,
            ));
            torso.add_child(&stripe);
        }

        // ── backpack and thrusters ──────────────────────────────────────────
        let mut pack = box_node(
            Vector3::new(
                spec.body_width * 0.8,
                spec.chest_height * 0.75,
                spec.body_depth * 0.5,
            ),
            &shell,
        );
        pack.set_position(Vector3::new(
            0.0,
            chest_y + spec.chest_height * 0.1,
            -spec.body_depth * 0.85,
        ));
        torso.add_child(&pack);

        let thruster_left = build_thruster(&mut torso, &dark, &visor, spec, chest_y, 1.0);
        let thruster_right = build_thruster(&mut torso, &dark, &visor, spec, chest_y, -1.0);

        // ── shoulders ───────────────────────────────────────────────────────
        let shoulder_y = chest_y + spec.chest_height * 0.24;
        for side in [1.0f32, -1.0] {
            let mut joint = box_node(
                Vector3::new(
                    spec.leg_width * 0.7,
                    spec.leg_width * 0.7,
                    spec.leg_width * 0.7,
                ),
                &dark,
            );
            joint.set_position(Vector3::new(side * spec.body_width * 0.58, shoulder_y, 0.0));
            torso.add_child(&joint);

            let mut pauldron = box_node(
                Vector3::new(
                    spec.body_width * 0.34,
                    spec.chest_height * 0.52,
                    spec.body_depth * 0.95,
                ),
                &shell,
            );
            pauldron.set_position(Vector3::new(
                side * spec.body_width * 0.78,
                shoulder_y + spec.chest_height * 0.10,
                0.0,
            ));
            // Angled, so the silhouette is not a rectangle on a rectangle.
            pauldron.set_rotation(Vector3::new(0.0, 0.0, side * -0.20));
            torso.add_child(&pauldron);

            let mut edge = box_node(
                Vector3::new(
                    spec.body_width * 0.36,
                    spec.chest_height * 0.10,
                    spec.body_depth * 0.97,
                ),
                &trim,
            );
            edge.set_position(Vector3::new(
                side * spec.body_width * 0.78,
                shoulder_y + spec.chest_height * 0.34,
                0.0,
            ));
            edge.set_rotation(Vector3::new(0.0, 0.0, side * -0.20));
            torso.add_child(&edge);
        }

        let gun = build_gun(&mut torso, &shell, &dark, &trim, spec, shoulder_y);
        let (blade_arm, blade) = build_blade(&mut torso, &shell, &dark, &visor, spec, shoulder_y);

        // ── head ────────────────────────────────────────────────────────────
        let mut head = Node3D::new_alloc();
        head.set_position(Vector3::new(0.0, chest_y + spec.chest_height * 0.95, 0.0));
        torso.add_child(&head);

        let mut skull = box_node(
            Vector3::new(
                spec.body_width * 0.38,
                spec.head_height,
                spec.body_depth * 0.66,
            ),
            &shell,
        );
        skull.set_position(Vector3::new(0.0, spec.head_height * 0.5, 0.0));
        head.add_child(&skull);

        let mut visor_slab = box_node(
            Vector3::new(
                spec.body_width * 0.28,
                spec.head_height * 0.14,
                spec.body_depth * 0.08,
            ),
            &visor,
        );
        visor_slab.set_position(Vector3::new(
            0.0,
            spec.head_height * 0.56,
            spec.body_depth * 0.34,
        ));
        head.add_child(&visor_slab);

        let mut antenna = box_node(
            Vector3::new(
                spec.body_width * 0.05,
                spec.head_height * 0.9,
                spec.body_width * 0.05,
            ),
            &trim,
        );
        antenna.set_position(Vector3::new(
            spec.body_width * 0.16,
            spec.head_height * 1.35,
            -spec.body_depth * 0.2,
        ));
        head.add_child(&antenna);

        parent.add_child(&root);

        Self {
            root,
            torso,
            head,
            left_leg,
            right_leg,
            left_knee,
            right_knee,
            left_foot,
            right_foot,
            gun,
            blade_arm,
            blade,
            thruster_left,
            thruster_right,
            shell,
            trim,
            spec,
        }
    }

    /// Pose the machine for this frame.
    pub fn pose(&mut self, pose: &MechPose) {
        self.root.set_position(to_godot(pose.at));
        self.root.set_rotation(Vector3::new(0.0, pose.yaw, 0.0));

        // The torso carries the turn lag and the lean. Both are read from the
        // simulation, which computed them from what already happened.
        self.torso.set_rotation(Vector3::new(
            pose.lean_x - pose.stagger * 0.30,
            pose.torso_yaw,
            pose.lean_z,
        ));
        let crouch = pose.stagger * self.spec.hip_height * 0.26;
        self.torso
            .set_position(Vector3::new(0.0, self.spec.hip_height - crouch, 0.0));

        // Gait: a sine pair in antiphase, so the legs are always opposite each
        // other. Airborne, both legs tuck instead.
        let swing = if pose.airborne {
            0.0
        } else {
            pose.gait.sin() * self.spec.stride
        };
        let lift = if pose.airborne {
            0.0
        } else {
            pose.gait.sin().abs() * 0.10
        };
        let tuck = if pose.airborne { 0.55 } else { 0.0 };

        self.left_leg.set_rotation(Vector3::new(swing, 0.0, 0.05));
        self.right_leg
            .set_rotation(Vector3::new(-swing, 0.0, -0.05));
        let bend = swing.abs() * 1.35 + lift + tuck;
        self.left_knee.set_rotation(Vector3::new(-bend, 0.0, 0.0));
        self.right_knee.set_rotation(Vector3::new(-bend, 0.0, 0.0));
        // The ankle gives most of the knee's bend back, so the foot stays flat
        // through the stride instead of pawing at the air.
        self.left_foot
            .set_rotation(Vector3::new(bend * 0.7 - tuck * 0.4, 0.0, 0.0));
        self.right_foot
            .set_rotation(Vector3::new(bend * 0.7 - tuck * 0.4, 0.0, 0.0));

        // The head holds the horizon: a mech that pitched its head with its
        // chest would look like it was falling over every time it leaned.
        self.head.set_rotation(Vector3::new(
            -pose.lean_x * 0.6 + pose.stagger * 0.45,
            0.0,
            0.0,
        ));

        self.gun.set_position(Vector3::new(
            0.0,
            0.0,
            -pose.recoil * self.spec.body_depth * 0.9,
        ));
        // The blade arm swings down through the cut, which is where the reach
        // comes from: the edge is swept across rather than thrust out.
        self.blade_arm
            .set_rotation(Vector3::new(pose.blade * 1.15, 0.0, 0.0));
        self.blade.set_scale(Vector3::new(
            1.0,
            1.0,
            (0.15 + pose.blade * 0.85).max(0.001),
        ));

        let burning = pose.thrust > 0.02;
        self.thruster_left.set_visible(burning);
        self.thruster_right.set_visible(burning);
        if burning {
            let flare = 0.6 + pose.thrust * 1.5;
            self.thruster_left.set_scale(Vector3::new(1.0, 1.0, flare));
            self.thruster_right.set_scale(Vector3::new(1.0, 1.0, flare));
        }

        // Hit flash: the shell runs hot for a fraction of a second. Applied to
        // the shared materials, which every part of this rig uses.
        let flash = pose.hit_flash.clamp(0.0, 1.0);
        self.shell
            .set_albedo(self.spec.shell_color.lerp(Color::WHITE, (flash * 0.8) as f64));
        self.shell
            .set("emission_enabled", &(flash > 0.01).to_variant());
        if flash > 0.01 {
            self.shell.set_emission(Color::WHITE);
            self.shell.set_emission_energy_multiplier(flash * 0.6);
        }
        self.trim.set_albedo(
            self.spec
                .trim_color
                .lerp(Color::WHITE, (flash * 0.8) as f64),
        );
    }

    /// Show or hide the whole machine.
    pub fn set_visible(&mut self, visible: bool) {
        self.root.set_visible(visible);
    }
}

/// One leg, returned with its knee and ankle pivots so all three can be posed.
fn build_leg(
    dark: &Gd<StandardMaterial3D>,
    shell: &Gd<StandardMaterial3D>,
    trim: &Gd<StandardMaterial3D>,
    spec: MechSpec,
    root: &mut Gd<Node3D>,
) -> (Gd<Node3D>, Gd<Node3D>, Gd<Node3D>) {
    let mut hip = Node3D::new_alloc();

    // Wide at the hip, narrow at the ankle: the shape follows the load.
    let thigh_length = spec.hip_height * 0.50;
    let mut thigh = box_node(
        Vector3::new(
            spec.leg_width * 1.15,
            thigh_length,
            spec.leg_width * 1.25,
        ),
        shell,
    );
    thigh.set_position(Vector3::new(0.0, -thigh_length * 0.5, 0.0));
    hip.add_child(&thigh);

    let mut knee = Node3D::new_alloc();
    knee.set_position(Vector3::new(0.0, -thigh_length, 0.0));
    hip.add_child(&knee);

    let mut knee_guard = box_node(
        Vector3::new(
            spec.leg_width * 0.95,
            spec.leg_width * 0.8,
            spec.leg_width * 0.5,
        ),
        trim,
    );
    knee_guard.set_position(Vector3::new(0.0, 0.0, spec.leg_width * 0.6));
    knee.add_child(&knee_guard);

    let shin_length = spec.hip_height * 0.42;
    let mut shin = box_node(
        Vector3::new(
            spec.leg_width * 0.9,
            shin_length,
            spec.leg_width * 1.05,
        ),
        shell,
    );
    shin.set_position(Vector3::new(0.0, -shin_length * 0.5, 0.0));
    knee.add_child(&shin);

    let mut ankle = Node3D::new_alloc();
    ankle.set_position(Vector3::new(0.0, -shin_length, 0.0));
    knee.add_child(&ankle);

    let mut foot = box_node(
        Vector3::new(
            spec.leg_width * 1.1,
            spec.hip_height * 0.07,
            spec.leg_width * 2.0,
        ),
        dark,
    );
    foot.set_position(Vector3::new(
        0.0,
        -spec.hip_height * 0.035,
        spec.leg_width * 0.4,
    ));
    ankle.add_child(&foot);

    root.add_child(&hip);
    (hip, knee, ankle)
}

fn build_thruster(
    torso: &mut Gd<Node3D>,
    dark: &Gd<StandardMaterial3D>,
    visor: &Gd<StandardMaterial3D>,
    spec: MechSpec,
    chest_y: f32,
    side: f32,
) -> Gd<Node3D> {
    let mut nozzle = Node3D::new_alloc();
    nozzle.set_position(Vector3::new(
        side * spec.body_width * 0.42,
        chest_y - spec.chest_height * 0.35,
        -spec.body_depth * 1.05,
    ));

    let mut housing = box_node(
        Vector3::new(
            spec.body_width * 0.26,
            spec.body_width * 0.26,
            spec.body_width * 0.4,
        ),
        dark,
    );
    housing.set_position(Vector3::new(0.0, 0.0, spec.body_width * 0.16));
    nozzle.add_child(&housing);

    let mut flame = box_node(
        Vector3::new(
            spec.body_width * 0.18,
            spec.body_width * 0.18,
            spec.body_width * 0.9,
        ),
        visor,
    );
    flame.set_position(Vector3::new(0.0, 0.0, -spec.body_width * 0.45));
    nozzle.add_child(&flame);

    nozzle.set_visible(false);
    torso.add_child(&nozzle);
    nozzle
}

fn build_gun(
    torso: &mut Gd<Node3D>,
    shell: &Gd<StandardMaterial3D>,
    dark: &Gd<StandardMaterial3D>,
    trim: &Gd<StandardMaterial3D>,
    spec: MechSpec,
    shoulder_y: f32,
) -> Gd<Node3D> {
    let mut arm = Node3D::new_alloc();
    arm.set_position(Vector3::new(spec.body_width * 0.72, shoulder_y, 0.0));

    let mut upper = box_node(
        Vector3::new(
            spec.leg_width * 0.8,
            spec.leg_width * 1.5,
            spec.leg_width * 0.85,
        ),
        shell,
    );
    upper.set_position(Vector3::new(0.0, -spec.leg_width * 0.85, 0.0));
    arm.add_child(&upper);

    let mut elbow = box_node(
        Vector3::new(
            spec.leg_width * 0.7,
            spec.leg_width * 0.7,
            spec.leg_width * 0.7,
        ),
        dark,
    );
    elbow.set_position(Vector3::new(0.0, -spec.leg_width * 1.75, 0.0));
    arm.add_child(&elbow);

    let barrel_length = spec.height * 0.40;
    let mut barrel = box_node(
        Vector3::new(
            spec.leg_width * 0.5,
            spec.leg_width * 0.5,
            barrel_length,
        ),
        dark,
    );
    barrel.set_position(Vector3::new(
        0.0,
        -spec.leg_width * 1.75,
        -barrel_length * 0.32,
    ));
    arm.add_child(&barrel);

    let mut shroud = box_node(
        Vector3::new(
            spec.leg_width * 0.72,
            spec.leg_width * 0.72,
            barrel_length * 0.42,
        ),
        shell,
    );
    shroud.set_position(Vector3::new(
        0.0,
        -spec.leg_width * 1.75,
        -barrel_length * 0.16,
    ));
    arm.add_child(&shroud);

    // A muzzle brake, so the end of the barrel has a shape rather than being
    // simply where the box stops.
    let mut muzzle = box_node(
        Vector3::new(
            spec.leg_width * 0.8,
            spec.leg_width * 0.8,
            spec.leg_width * 0.45,
        ),
        trim,
    );
    muzzle.set_position(Vector3::new(
        0.0,
        -spec.leg_width * 1.75,
        -barrel_length * 0.88,
    ));
    arm.add_child(&muzzle);

    torso.add_child(&arm);
    arm
}

fn build_blade(
    torso: &mut Gd<Node3D>,
    shell: &Gd<StandardMaterial3D>,
    dark: &Gd<StandardMaterial3D>,
    visor: &Gd<StandardMaterial3D>,
    spec: MechSpec,
    shoulder_y: f32,
) -> (Gd<Node3D>, Gd<Node3D>) {
    let mut arm = Node3D::new_alloc();
    arm.set_position(Vector3::new(-spec.body_width * 0.72, shoulder_y, 0.0));

    let mut upper = box_node(
        Vector3::new(
            spec.leg_width * 0.8,
            spec.leg_width * 1.5,
            spec.leg_width * 0.85,
        ),
        shell,
    );
    upper.set_position(Vector3::new(0.0, -spec.leg_width * 0.85, 0.0));
    arm.add_child(&upper);

    let mut elbow = box_node(
        Vector3::new(
            spec.leg_width * 0.7,
            spec.leg_width * 0.7,
            spec.leg_width * 0.7,
        ),
        dark,
    );
    elbow.set_position(Vector3::new(0.0, -spec.leg_width * 1.75, 0.0));
    arm.add_child(&elbow);

    let mut emitter = box_node(
        Vector3::new(
            spec.leg_width * 0.55,
            spec.leg_width * 0.55,
            spec.height * 0.17,
        ),
        dark,
    );
    emitter.set_position(Vector3::new(
        0.0,
        -spec.leg_width * 1.75,
        -spec.height * 0.085,
    ));
    arm.add_child(&emitter);

    let mut blade = box_node(
        Vector3::new(
            spec.leg_width * 0.14,
            spec.leg_width * 1.35,
            spec.height * 0.60,
        ),
        visor,
    );
    blade.set_position(Vector3::new(
        0.0,
        -spec.leg_width * 1.75,
        -spec.height * 0.17 - spec.height * 0.30,
    ));
    arm.add_child(&blade);

    torso.add_child(&arm);
    (arm, blade.upcast())
}
