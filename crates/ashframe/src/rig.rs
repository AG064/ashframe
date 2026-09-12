//! Mech visuals.
//!
//! Every mech is a Blender model — built by the scripts in the Blender MCP's
//! `parts/` directory and exported as glTF — instanced into the scene and posed
//! from simulation state every frame. There is no animation clip and no skinned
//! mesh: the pose is arithmetic over the same numbers the simulation already
//! publishes, applied to the named joints the model exports.
//!
//! That is what keeps the renderer honest. It cannot show a mech doing
//! something the simulation is not doing, because it has nothing else to read,
//! and it is why replacing the models did not touch a line of gameplay: the
//! joints are the same joints the procedural rig had, so the arithmetic that
//! drove those drives these.
//!
//! ## The joint names are an interface
//!
//! `torso`, `head`, `leg_l`, `knee_r`, `gun_l` and the rest come from
//! `parts/mechs.py` in the Blender MCP. Changing one there without changing it
//! here gives a mech that stands still and a warning in the log, which is the
//! failure mode to prefer over a mech that silently animates the wrong limb.

use godot::classes::{Node, Node3D, PackedScene, ResourceLoader};
use godot::prelude::*;

use ashframe_sim::types::{EnemyKind, Vec3};

use crate::palette::to_godot;

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

impl Frame {
    /// The glTF file, and the prefix its joints carry.
    ///
    /// The prefix is the builder's short name for the machine, which is what
    /// keeps two mechs loaded at once from finding each other's joints.
    fn model(&self) -> (&'static str, &'static str) {
        match self {
            Self::Player => ("res://models/ashframe.glb", "ASH"),
            Self::Hostile(EnemyKind::Skirmisher) => ("res://models/skirmisher.glb", "SKM"),
            Self::Hostile(EnemyKind::Artillery) => ("res://models/artillery.glb", "ART"),
            Self::Hostile(EnemyKind::Boss) => ("res://models/patriarch.glb", "PAT"),
        }
    }

    /// How tall the machine stands, for the camera and the HUD.
    pub fn height(&self) -> f32 {
        match self {
            Self::Player => ashframe_sim::config::player::HEIGHT,
            Self::Hostile(EnemyKind::Skirmisher) => ashframe_sim::config::skirmisher::HEIGHT,
            Self::Hostile(EnemyKind::Artillery) => ashframe_sim::config::artillery::HEIGHT,
            Self::Hostile(EnemyKind::Boss) => ashframe_sim::config::boss::HEIGHT,
        }
    }
}

/// One articulated mech, loaded from a model.
pub struct MechRig {
    pub root: Gd<Node3D>,
    torso: Option<Gd<Node3D>>,
    head: Option<Gd<Node3D>>,
    hips: [Option<Gd<Node3D>>; 2],
    knees: [Option<Gd<Node3D>>; 2],
    feet: [Option<Gd<Node3D>>; 2],
    guns: [Option<Gd<Node3D>>; 2],
    blades: [Option<Gd<Node3D>>; 2],
    /// How tall the machine stands. The model is authored at this size, so it
    /// is used for proportions rather than for scaling.
    height: f32,
    /// Where the joints sit in the model, before anything is posed.
    rest: Rest,
}

/// The authored position of every joint a pose moves.
///
/// A pose *offsets* a joint rather than placing it. Setting a joint's position
/// outright moves it to the origin of whatever it hangs from, and for the torso
/// that origin is the root — which is the ground. The first version of this
/// file did exactly that, and the machine's chest ended up below its own feet.
#[derive(Debug, Clone, Copy, Default)]
struct Rest {
    torso: Vector3,
    guns: [Vector3; 2],
}

impl MechRig {
    /// Load a mech and parent it.
    ///
    /// Returns `None` when the model is missing, rather than panicking: a game
    /// that cannot find one of its meshes should be obviously wrong on screen,
    /// not refuse to start.
    pub fn new(parent: &mut Gd<Node3D>, frame: Frame) -> Option<Self> {
        let (path, prefix) = frame.model();
        let scene = load_scene(path)?;
        let Some(mut root) = scene
            .instantiate()
            .and_then(|node| node.try_cast::<Node3D>().ok())
        else {
            godot::global::godot_error!("{path} did not instantiate as a 3D node");
            return None;
        };
        let name = format!("rig_{prefix}");
        root.set_name(name.as_str());
        parent.add_child(&root);

        let joint = |root: &Gd<Node3D>, name: &str| -> Option<Gd<Node3D>> {
            let full = format!("{prefix}_{name}");
            let node: Gd<Node> = root.clone().upcast();
            // `find_child` searches the whole subtree and matches nodes the
            // scene owns, which is what an instantiated model's joints are.
            match node.find_child(full.as_str()) {
                Some(found) => found.try_cast::<Node3D>().ok(),
                None => {
                    // Named rather than silent. A renamed joint in the Blender
                    // builder is the one way this file and the model can
                    // disagree, and it should be loud.
                    godot::global::godot_warn!("{path} has no joint named {full}");
                    None
                }
            }
        };

        let mut rig = Self {
            torso: joint(&root, "torso"),
            head: joint(&root, "head"),
            hips: [joint(&root, "leg_l"), joint(&root, "leg_r")],
            knees: [joint(&root, "knee_l"), joint(&root, "knee_r")],
            feet: [joint(&root, "foot_l"), joint(&root, "foot_r")],
            guns: [joint(&root, "gun_l"), joint(&root, "gun_r")],
            blades: [joint(&root, "blade_l"), joint(&root, "blade_r")],
            root,
            height: frame.height(),
            rest: Rest::default(),
        };
        // Captured before anything is posed, because a pose *offsets* these
        // joints rather than placing them.
        rig.rest = Rest {
            torso: rig
                .torso
                .as_ref()
                .map(|n| n.get_position())
                .unwrap_or_default(),
            guns: [
                rig.guns[0]
                    .as_ref()
                    .map(|n| n.get_position())
                    .unwrap_or_default(),
                rig.guns[1]
                    .as_ref()
                    .map(|n| n.get_position())
                    .unwrap_or_default(),
            ],
        };
        Some(rig)
    }

    /// Pose the machine for this frame.
    pub fn pose(&mut self, pose: &MechPose) {
        self.root.set_position(to_godot(pose.at));
        self.root.set_rotation(Vector3::new(0.0, pose.yaw, 0.0));

        // The torso carries the turn lag and the lean. Both are read from the
        // simulation, which computed them from what already happened.
        if let Some(torso) = self.torso.as_mut() {
            torso.set_rotation(Vector3::new(
                pose.lean_x - pose.stagger * 0.30,
                pose.torso_yaw,
                pose.lean_z,
            ));
            // A stagger drops the body onto its hips, which is most of what
            // sells being hit hard. Added to where the model put the joint, not
            // put in place of it.
            torso.set_position(
                self.rest.torso + Vector3::new(0.0, -pose.stagger * self.height * 0.09, 0.0),
            );
        }

        // Gait: a sine pair in antiphase, so the legs are always opposite each
        // other. Airborne, both legs tuck instead.
        let swing = if pose.airborne {
            0.0
        } else {
            pose.gait.sin() * 0.50
        };
        let lift = if pose.airborne {
            0.0
        } else {
            pose.gait.sin().abs() * 0.10
        };
        let tuck = if pose.airborne { 0.55 } else { 0.0 };
        let bend = swing.abs() * 1.35 + lift + tuck;

        for (index, sign) in [(0usize, 1.0f32), (1, -1.0)] {
            if let Some(hip) = self.hips[index].as_mut() {
                hip.set_rotation(Vector3::new(sign * swing, 0.0, sign * 0.05));
            }
            if let Some(knee) = self.knees[index].as_mut() {
                knee.set_rotation(Vector3::new(-bend, 0.0, 0.0));
            }
            // The ankle gives most of the knee's bend back, so the foot stays
            // flat through the stride instead of pawing at the air.
            if let Some(foot) = self.feet[index].as_mut() {
                foot.set_rotation(Vector3::new(bend * 0.7 - tuck * 0.4, 0.0, 0.0));
            }
        }

        // The head holds the horizon: a mech that pitched its head with its
        // chest would look like it was falling over every time it leaned.
        if let Some(head) = self.head.as_mut() {
            head.set_rotation(Vector3::new(
                -pose.lean_x * 0.6 + pose.stagger * 0.45,
                0.0,
                0.0,
            ));
        }

        for (index, node) in self.guns.iter_mut().enumerate() {
            if let Some(gun) = node.as_mut() {
                gun.set_position(
                    self.rest.guns[index]
                        + Vector3::new(0.0, 0.0, -pose.recoil * self.height * 0.04),
                );
            }
        }

        // The blade arm swings down through the cut, which is where the reach
        // comes from: the edge is swept across rather than thrust out.
        for node in self.blades.iter_mut() {
            if let Some(blade) = node.as_mut() {
                blade.set_rotation(Vector3::new(pose.blade * 1.15, 0.0, 0.0));
            }
        }
    }

    /// Show or hide the whole machine.
    pub fn set_visible(&mut self, visible: bool) {
        self.root.set_visible(visible);
    }
}

/// Load a glTF into a packed scene.
///
/// `ResourceLoader` is used rather than a preloaded `PackedScene` field because
/// the models are content: replacing one should be a rebuild of the asset, not
/// a rebuild of the extension that reads it.
pub fn load_scene(path: &str) -> Option<Gd<PackedScene>> {
    let mut loader = ResourceLoader::singleton();
    if !loader.exists(path) {
        godot::global::godot_warn!("missing model: {path}");
        return None;
    }
    let resource = loader.load(path)?;
    match resource.try_cast::<PackedScene>() {
        Ok(scene) => Some(scene),
        Err(_) => {
            godot::global::godot_error!("{path} is not a packed scene");
            None
        }
    }
}

/// Load a model and parent it, for things that are placed rather than posed.
pub fn load_prop(parent: &mut Gd<Node3D>, path: &str, name: &str) -> Option<Gd<Node3D>> {
    let scene = load_scene(path)?;
    let mut root = scene
        .instantiate()
        .and_then(|node| node.try_cast::<Node3D>().ok())?;
    root.set_name(name);
    parent.add_child(&root);
    Some(root)
}

/// Find a node by name anywhere under a root.
pub fn child_named(root: &Gd<Node3D>, name: &str) -> Option<Gd<Node3D>> {
    let node: Gd<Node> = root.clone().upcast();
    match node.find_child(name) {
        Some(found) => found.try_cast::<Node3D>().ok(),
        None => None,
    }
}
