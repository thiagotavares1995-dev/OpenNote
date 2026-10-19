//! Loads and rigs the two hand models, colours their skin, and poses them every frame from
//! the timeline.

use std::collections::HashMap;

use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::VertexAttributeValues;
use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use on_hand::skeleton::Joint;
use on_hand::{Finger, Hand};
use tracing::warn;

use crate::render::{Performance, Transport, KEYBOARD_Y_MM};
use crate::rig::{self, BoneRole};

#[allow(dead_code)]
fn wear_gloves(material: &mut StandardMaterial, cloth: Option<Handle<Image>>) {
    material.base_color = GLOVE_YELLOW;
    material.normal_map_texture = cloth;
    material.perceptual_roughness = 0.78;
    material.reflectance = 0.16;
    material.metallic = 0.0;
    material.double_sided = true;
    material.cull_mode = None;
    material.diffuse_transmission = 0.08;
    material.thickness = 8.0;
}

#[allow(dead_code)]
const GLOVE_YELLOW: Color = Color::srgb(0.97, 0.78, 0.11);
const BARE_SKIN: Color = Color::srgb(0.87, 0.69, 0.58);

const BACK_OF_HAND: Color = Color::srgb(0.84, 0.65, 0.54);
const PALM: Color = Color::srgb(0.90, 0.70, 0.63);
const FOREARM: Color = Color::srgb(0.83, 0.64, 0.52);
const KNUCKLE: Color = Color::srgb(0.76, 0.52, 0.45);
const CREASE: Color = Color::srgb(0.74, 0.50, 0.46);
const FINGERTIP: Color = Color::srgb(0.90, 0.60, 0.55);
const NAIL: Color = Color::srgb(0.93, 0.76, 0.73);

const KNUCKLE_REACH: [f32; 3] = [0.13, 0.085, 0.065];
const CREASE_REACH: f32 = 0.022;
const MOTTLE: f32 = 0.045;

type Skin = ExtendedMaterial<StandardMaterial, Flesh>;

#[derive(Asset, TypePath, bevy::render::render_resource::AsBindGroup, Clone)]
struct Flesh {
    #[uniform(100)]
    tone: FleshTone,
}

#[derive(bevy::render::render_resource::ShaderType, Clone)]
struct FleshTone {
    scatter: Vec4,
    wrap: Vec4,
    sheen: Vec4,
}

impl MaterialExtension for Flesh {
    fn fragment_shader() -> bevy::shader::ShaderRef {
        "embedded://on_viz/hand.wgsl".into()
    }
}

fn is_arm_bone(name: &str) -> bool {
    let lowered = name.to_ascii_lowercase();
    lowered.contains("lowerarm") || lowered.contains("forearm") || lowered.contains("upperarm")
}

struct Digit {
    finger: Finger,
    bones: Vec<DrivenBone>,
}

struct DrivenBone {
    entity: Entity,
    rest_rotation: Quat,
    rest_translation: Vec3,
}

#[derive(Component)]
pub struct HandRig {
    hand: Hand,
    correction: Quat,
    wrist_offset: Vec3,
    wrist_rotation: Quat,
    scale: f32,
    /// How long the model's palm is, wrist to middle knuckle, so that the hand can be
    /// scaled again when the pianist's hand size changes.
    palm: f32,
    digits: Vec<Digit>,
    forearm: Option<Forearm>,
}

#[derive(Clone)]
struct Forearm {
    top: Entity,
    top_rest: Quat,
    top_parent: Quat,
    head: Vec3,
    wrist: Entity,
    wrist_rest: Quat,
}

impl Forearm {
    fn turned(&self, turn: Quat, wrist_in_model: Quat) -> (Quat, Quat) {
        let wrist_parent = wrist_in_model * self.wrist_rest.inverse();
        (
            self.top_parent.inverse() * turn * self.top_parent * self.top_rest,
            wrist_parent.inverse() * turn.inverse() * wrist_parent * self.wrist_rest,
        )
    }
}

const FOREARM_TURN_MAX_DEG: f32 = 30.0;

const LEAN_SIGMA_SECONDS: f64 = 0.6;

const LEAN_SAMPLES: i32 = 6;

fn settled_wrist(animator: &crate::HandAnimator, time: f64) -> Vec3 {
    let step = LEAN_SIGMA_SECONDS / 3.0;
    let (mut sum, mut total) = (Vec3::ZERO, 0.0f32);
    for k in -LEAN_SAMPLES..=LEAN_SAMPLES {
        let weight = (-0.5 * (f64::from(k) * step / LEAN_SIGMA_SECONDS).powi(2)).exp() as f32;
        sum += animator.unpressed(time + f64::from(k) * step).wrist_position() * weight;
        total += weight;
    }
    sum / total
}

#[derive(Component)]
struct NeedsRigging {
    hand: Hand,
}

#[derive(Component)]
struct HandMesh;

pub struct HandsPlugin;

impl Plugin for HandsPlugin {
    fn build(&self, app: &mut App) {
        bevy::asset::embedded_asset!(app, "hand.wgsl");
        app.add_plugins(MaterialPlugin::<Skin>::default())
            .add_systems(Startup, setup_hands)
            .add_systems(Update, (rig_loaded_hands, prepare_hand_meshes, rescale_hands.before(pose_hands)));
    }
}

fn setup_hands(mut commands: Commands, assets: Res<AssetServer>, performance: Res<Performance>) {
    if !performance.hands_available {
        warn!(
            "no rigged hand models found, so the keyboard will play without hands; \
             they ship in the repository's assets/hands folder"
        );
        return;
    }
    for hand in Hand::ALL {
        let file = match hand {
            Hand::Left => "hands/hand-left.glb",
            Hand::Right => "hands/hand-right.glb",
        };
        let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(file));
        commands.spawn((
            WorldAssetRoot(scene),
            Transform::IDENTITY,
            HandRig {
                hand,
                correction: Quat::IDENTITY,
                wrist_offset: Vec3::ZERO,
                wrist_rotation: Quat::IDENTITY,
                scale: 1.0,
                palm: 0.0,
                digits: Vec::new(),
                forearm: None,
            },
            NeedsRigging { hand },
        ));
    }
}

struct Survey {
    nodes: HashMap<Entity, Node>,
    roles: HashMap<BoneRole, Entity>,
    arms: std::collections::HashSet<Entity>,
}

struct Node {
    parent: Option<Entity>,
    local: Transform,
    world: Vec3,
}

fn rest_in_model(survey: &Survey, entity: Option<Entity>) -> (Vec3, Quat) {
    let (mut position, mut rotation) = (Vec3::ZERO, Quat::IDENTITY);
    let mut walker = entity;
    while let Some(node) = walker.and_then(|e| survey.nodes.get(&e)) {
        position = node.local.transform_point(position);
        rotation = node.local.rotation * rotation;
        walker = node.parent;
    }
    (position, rotation)
}

fn find_forearm(survey: &Survey, wrist: Entity) -> Option<Forearm> {
    let mut top = None;
    let mut at = survey.nodes.get(&wrist)?.parent;
    while let Some(entity) = at.filter(|e| survey.arms.contains(e)) {
        top = Some(entity);
        at = survey.nodes.get(&entity)?.parent;
    }
    let top = top?;
    let node = survey.nodes.get(&top)?;
    Some(Forearm {
        top,
        top_rest: node.local.rotation,
        top_parent: rest_in_model(survey, node.parent).1,
        head: rest_in_model(survey, Some(top)).0,
        wrist,
        wrist_rest: survey.nodes.get(&wrist)?.local.rotation,
    })
}

fn rig_loaded_hands(
    mut commands: Commands,
    performance: Res<Performance>,
    pending: Query<(Entity, &NeedsRigging)>,
    names: Query<(&Name, &Transform)>,
    parents: Query<&ChildOf>,
    globals: Query<&GlobalTransform>,
    children: Query<&Children>,
    mut rigs: Query<&mut HandRig>,
) {
    for (root, needs) in &pending {
        let Some(survey) = survey_scene(root, needs.hand, &children, &names, &parents, &globals) else {
            continue;
        };
        let Some(wrist) = survey.roles.get(&BoneRole::Wrist).copied() else {
            warn!("the {:?} hand rig has no wrist bone", needs.hand);
            commands.entity(root).remove::<NeedsRigging>();
            continue;
        };

        let Ok(root_global) = globals.get(root) else {
            continue;
        };
        let inverse = root_global.affine().inverse();
        let local =
            |entity: Entity| survey.nodes.get(&entity).map(|node| inverse.transform_point3(node.world));

        let Some(fit) = measure_fit(needs.hand, &survey, &local, &performance) else {
            commands.entity(root).remove::<NeedsRigging>();
            continue;
        };
        let digits = build_digits(&survey, wrist);
        if digits.is_empty() {
            warn!("the {:?} hand rig has no finger bones", needs.hand);
            commands.entity(root).remove::<NeedsRigging>();
            continue;
        }

        let found: usize = digits.iter().map(|d| d.bones.len()).sum();
        debug!(
            "the {:?} hand rig has {} digits and {found} driven bones, scale {:.1}",
            needs.hand,
            digits.len(),
            fit.scale
        );

        if let Ok(mut rig) = rigs.get_mut(root) {
            rig.correction = fit.correction;
            rig.wrist_offset = fit.wrist_offset;
            rig.wrist_rotation = fit.wrist_rotation;
            rig.scale = fit.scale;
            rig.palm = fit.palm;
            rig.digits = digits;
            rig.forearm = find_forearm(&survey, wrist);
        }
        commands.entity(root).remove::<NeedsRigging>();
    }
}

fn survey_scene(
    root: Entity,
    hand: Hand,
    children: &Query<&Children>,
    names: &Query<(&Name, &Transform)>,
    parents: &Query<&ChildOf>,
    globals: &Query<&GlobalTransform>,
) -> Option<Survey> {
    let mut survey =
        Survey { nodes: HashMap::new(), roles: HashMap::new(), arms: std::collections::HashSet::new() };
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
        let Ok((name, local)) = names.get(entity) else {
            continue;
        };
        let Ok(global) = globals.get(entity) else {
            continue;
        };
        survey.nodes.insert(
            entity,
            Node {
                parent: parents.get(entity).ok().map(|p| p.parent()),
                local: *local,
                world: global.translation(),
            },
        );
        if is_arm_bone(name.as_str()) {
            survey.arms.insert(entity);
        }
        if let Some(bone) = rig::classify(name.as_str()) {
            if bone.hand == hand {
                survey.roles.insert(bone.role, entity);
            }
        }
    }
    (!survey.roles.is_empty()).then_some(survey)
}

fn build_digits(survey: &Survey, wrist: Entity) -> Vec<Digit> {
    let mut digits = Vec::new();
    for finger in Finger::ALL {
        let (Some(&proximal), Some(&intermediate), Some(&distal)) = (
            survey.roles.get(&BoneRole::Proximal(finger)),
            survey.roles.get(&BoneRole::Intermediate(finger)),
            survey.roles.get(&BoneRole::Distal(finger)),
        ) else {
            continue;
        };

        let mut metacarpal = None;
        let mut walker = survey.nodes.get(&proximal).and_then(|node| node.parent);
        while let Some(entity) = walker {
            if entity == wrist {
                break;
            }
            if metacarpal.is_none() {
                metacarpal = Some(entity);
            }
            walker = survey.nodes.get(&entity).and_then(|node| node.parent);
        }
        if walker.is_none() {
            continue;
        }

        let mut bones = Vec::with_capacity(4);
        for entity in metacarpal.into_iter().chain([proximal, intermediate, distal]) {
            let Some(node) = survey.nodes.get(&entity) else {
                continue;
            };
            bones.push(DrivenBone {
                entity,
                rest_rotation: node.local.rotation,
                rest_translation: node.local.translation,
            });
        }
        digits.push(Digit { finger, bones });
    }
    digits
}

struct RigFit {
    correction: Quat,
    wrist_offset: Vec3,
    wrist_rotation: Quat,
    scale: f32,
    palm: f32,
}

fn palm_length(skeleton: &on_hand::Skeleton) -> f32 {
    let posture = skeleton.forward(&skeleton.rest_pose(glam::Vec3::ZERO));
    (posture.joint(Joint::Base(Finger::Middle)) - posture.wrist).length()
}

/// The hands are scaled to the pianist's hand when they are rigged; a different hand
/// size chosen later rescales them to match.
fn rescale_hands(performance: Res<Performance>, mut rigs: Query<&mut HandRig>) {
    if !performance.is_changed() {
        return;
    }
    for mut rig in &mut rigs {
        if rig.palm > 1e-6 {
            let scale = palm_length(performance.animators[rig.hand as usize].skeleton()) / rig.palm;
            if (scale - rig.scale).abs() > 1e-6 {
                rig.scale = scale;
            }
        }
    }
}

fn measure_fit(
    hand: Hand,
    survey: &Survey,
    local: &impl Fn(Entity) -> Option<Vec3>,
    performance: &Performance,
) -> Option<RigFit> {
    let at = |role: BoneRole| survey.roles.get(&role).copied().and_then(local);

    let (Some(wrist), Some(tip), Some(index), Some(little), Some(middle_knuckle)) = (
        at(BoneRole::Wrist),
        at(BoneRole::Distal(Finger::Middle)),
        at(BoneRole::Proximal(Finger::Index)),
        at(BoneRole::Proximal(Finger::Little)),
        at(BoneRole::Proximal(Finger::Middle)),
    ) else {
        warn!("the {hand:?} hand rig is missing the bones needed to orient it");
        return None;
    };
    let correction = rig::rest_correction(hand, wrist, tip, index, little)?;

    let measured = (middle_knuckle - wrist).length();
    let wanted = palm_length(performance.animators[hand as usize].skeleton());
    let scale = if measured > 1e-6 { wanted / measured } else { 1.0 };

    let wrist_entity = *survey.roles.get(&BoneRole::Wrist)?;
    let mut wrist_offset = Vec3::ZERO;
    let mut wrist_rotation = Quat::IDENTITY;
    let mut walker = Some(wrist_entity);
    while let Some(node) = walker.and_then(|entity| survey.nodes.get(&entity)) {
        wrist_offset = node.local.transform_point(wrist_offset);
        wrist_rotation = node.local.rotation * wrist_rotation;
        walker = node.parent;
    }

    Some(RigFit { correction, wrist_offset, wrist_rotation, scale, palm: measured })
}

fn prepare_hand_meshes(
    mut commands: Commands,
    mut materials: ResMut<Assets<Skin>>,
    mut meshes: ResMut<Assets<Mesh>>,
    poses: Res<Assets<SkinnedMeshInverseBindposes>>,
    names: Query<&Name>,
    skinned: Query<(Entity, &Mesh3d, &SkinnedMesh), Without<HandMesh>>,
) {
    for (entity, mesh, skin) in &skinned {
        let bones: Vec<(String, Vec3)> = poses
            .get(&skin.inverse_bindposes)
            .map(|poses| {
                skin.joints
                    .iter()
                    .zip(poses.iter())
                    .map(|(joint, pose)| {
                        let name = names
                            .get(*joint)
                            .map(|name| name.as_str().to_ascii_lowercase())
                            .unwrap_or_default();
                        (name, pose.inverse().transform_point3(Vec3::ZERO))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tinted = meshes.get_mut(&mesh.0).is_some_and(|mut mesh| {
            complexion(&mesh, &bones)
                .map(|colours| mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colours))
                .is_some()
        });
        commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>().insert((
            HandMesh,
            NoFrustumCulling,
            bevy::light::NotShadowReceiver,
            MeshMaterial3d(materials.add(bare_hand(tinted))),
        ));
    }
}

fn bare_hand(tinted: bool) -> Skin {
    ExtendedMaterial {
        base: StandardMaterial {
            base_color: if tinted { Color::WHITE } else { BARE_SKIN },
            perceptual_roughness: 0.56,
            reflectance: 0.40,
            diffuse_transmission: 0.12,
            thickness: 6.0,
            double_sided: true,
            cull_mode: None,
            ..default()
        },
        extension: Flesh {
            tone: FleshTone {
                scatter: Vec4::new(1.0, 0.32, 0.20, 1.0),
                wrap: Vec4::new(0.60, 0.24, 0.13, 0.55),
                sheen: Vec4::new(0.10, 0.10, 0.12, 0.24),
            },
        },
    }
}

fn complexion(mesh: &Mesh, bones: &[(String, Vec3)]) -> Option<Vec<[f32; 4]>> {
    let Some(VertexAttributeValues::Float32x3(positions)) = mesh.attribute(Mesh::ATTRIBUTE_POSITION) else {
        return None;
    };
    let Some(VertexAttributeValues::Float32x3(normals)) = mesh.attribute(Mesh::ATTRIBUTE_NORMAL) else {
        return None;
    };
    let Some(VertexAttributeValues::Float32x4(weights)) = mesh.attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT) else {
        return None;
    };
    let joints: Vec<[u16; 4]> = match mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX)? {
        VertexAttributeValues::Uint16x4(values) => values.clone(),
        VertexAttributeValues::Uint8x4(values) => values.iter().map(|v| v.map(u16::from)).collect(),
        _ => return None,
    };

    let head = |wanted: &str| bones.iter().find(|(name, _)| name.starts_with(wanted)).map(|(_, at)| *at);
    let frame = crate::glove::HandFrame::new(
        head("wrist")?,
        head("finger3-1")?,
        head("finger2-1")?,
        head("finger5-1")?,
        head("finger1-3")?,
    );

    let segment = |name: &str| -> Option<(usize, usize)> {
        let rest = name.strip_prefix("finger")?;
        let finger = rest.get(0..1)?.parse::<usize>().ok()?;
        let bone = rest.get(2..3)?.parse::<usize>().ok()?;
        (rest.get(1..2) == Some("-")).then_some((finger, bone))
    };
    let knuckles: Vec<(Vec3, Vec3, f32)> = bones
        .iter()
        .filter_map(|(name, at)| {
            let (finger, bone) = segment(name)?;
            let next = head(&format!("finger{finger}-{}", bone + 1));
            let previous = head(&format!("finger{finger}-{}", bone.checked_sub(1)?));
            let axis = match (next, previous) {
                (Some(next), _) => next - *at,
                (None, Some(previous)) => *at - previous,
                _ => return None,
            };
            let reach = KNUCKLE_REACH[(bone.max(1) - 1).min(2)] * frame.palm;
            Some((*at, axis.normalize_or_zero(), reach))
        })
        .collect();

    let thumb_nail = (frame.dorsal * 0.5 - frame.across * 0.85).normalize_or_zero();
    let distal: Vec<Option<(Vec3, Vec3, f32, Vec3)>> = bones
        .iter()
        .map(|(name, at)| {
            let (finger, bone) = segment(name)?;
            if bone != 3 {
                return None;
            }
            let previous = head(&format!("finger{finger}-2"))?;
            let length = (*at - previous).length() * 0.9;
            let nail = if finger == 1 { thumb_nail } else { frame.dorsal };
            Some((*at, (*at - previous).normalize_or_zero(), length.max(1e-6), nail))
        })
        .collect();
    let on_arm: Vec<bool> = bones.iter().map(|(name, _)| is_arm_bone(name)).collect();

    let linear = |colour: Color| {
        let colour = colour.to_linear();
        Vec3::new(colour.red, colour.green, colour.blue)
    };
    let mut out = Vec::with_capacity(positions.len());
    for i in 0..positions.len() {
        let point = Vec3::from(positions[i]);
        let normal = Vec3::from(normals[i]).normalize_or_zero();
        let facing = normal.dot(frame.dorsal);

        let (mut arm, mut tip, mut nail) = (0.0, 0.0, 0.0);
        for (joint, weight) in joints[i].iter().zip(weights[i]) {
            let joint = *joint as usize;
            if on_arm.get(joint).copied().unwrap_or(false) {
                arm += weight;
            }
            if let Some(Some((at, axis, length, up))) = distal.get(joint) {
                let along = (point - *at).dot(*axis) / length;
                tip += weight * smooth(0.15, 0.85, along);
                nail += weight * smooth(0.30, 0.48, along) * smooth(0.40, 0.72, normal.dot(*up));
            }
        }

        let (mut knuckle, mut crease) = (0.0f32, 0.0f32);
        for (at, axis, reach) in &knuckles {
            let offset = point - *at;
            let along = offset.dot(*axis);
            let distance = offset.length() / reach;
            knuckle = knuckle.max((-distance * distance).exp());
            let sideways = (offset - *axis * along).length() / reach;
            if sideways < 1.6 {
                let line = along / (CREASE_REACH * frame.palm);
                crease = crease.max((-line * line).exp());
            }
        }
        knuckle *= smooth(0.1, 0.6, facing);
        crease *= smooth(0.2, 0.7, -facing);

        let mut colour = linear(BACK_OF_HAND).lerp(linear(PALM), smooth(-0.1, 0.6, -facing));
        colour = colour.lerp(linear(FOREARM), arm.clamp(0.0, 1.0));
        colour = colour.lerp(linear(FINGERTIP), tip.clamp(0.0, 1.0) * 0.8);
        colour = colour.lerp(linear(KNUCKLE), knuckle * 0.7);
        colour = colour.lerp(linear(CREASE), crease * 0.6);
        let nail = nail.clamp(0.0, 1.0);
        colour = colour.lerp(linear(NAIL), nail);

        let scaled = point / frame.palm;
        let mottle = (scaled.x * 17.0).sin() * (scaled.y * 13.0 + 1.3).sin() * (scaled.z * 19.0 + 0.7).sin();
        colour *= 1.0 + MOTTLE * mottle;
        out.push([colour.x, colour.y, colour.z, 1.0 - nail]);
    }
    Some(out)
}

fn smooth(from: f32, to: f32, x: f32) -> f32 {
    let t = ((x - from) / (to - from)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[allow(dead_code)]
fn hand_frame(
    joints: &[Entity],
    names: &Query<&Name>,
    places: &Query<&GlobalTransform>,
) -> Option<crate::glove::HandFrame> {
    let find = |wanted: &str| {
        joints
            .iter()
            .find(|joint| {
                names.get(**joint).is_ok_and(|name| name.as_str().to_ascii_lowercase().contains(wanted))
            })
            .and_then(|joint| places.get(*joint).ok())
            .map(|place| place.translation())
    };
    Some(crate::glove::HandFrame::new(
        find("wrist")?,
        find("finger3-1")?,
        find("finger2-1")?,
        find("finger5-1")?,
        find("finger1-3")?,
    ))
}

pub fn pose_hands(
    transport: Res<Transport>,
    performance: Res<Performance>,
    playing: Res<crate::render::Playing>,
    rigs: Query<(Entity, &HandRig)>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
) {
    medir!("pose_hands");
    let offset = Vec3::Y * KEYBOARD_Y_MM;
    let poses = crate::timeline::pose_both_decided(
        &performance.animators,
        &performance.decisions,
        transport.position,
    );

    for (root, rig) in &rigs {
        let shown = playing.0[rig.hand as usize];
        if let Ok(mut visibility) = visibility.get_mut(root) {
            visibility.set_if_neq(if shown { Visibility::Inherited } else { Visibility::Hidden });
        }
        if rig.digits.is_empty() || !shown {
            continue;
        }
        let animator = &performance.animators[rig.hand as usize];
        let pose = poses[rig.hand as usize];
        let skeleton = animator.skeleton();
        let posture = skeleton.forward(&pose);

        let root_rotation = skeleton.wrist_rotation(&pose) * rig.correction;
        let wrist_world = posture.wrist + offset;

        let mut wrist_in_model = rig.wrist_offset;
        if let Some(arm) = rig.forearm.as_ref() {
            let torso = &performance.torso;
            let settled = settled_wrist(animator, transport.position);
            let lean = torso.lean_for(rig.hand, settled);
            let shoulder = torso.shoulder(rig.hand, lean);
            let elbow = torso.elbow(rig.hand, shoulder, posture.wrist);
            let along = posture.wrist - elbow;
            let wanted = Vec3::new(along.x, along.y, 0.0).normalize_or_zero();
            let rest_direction = (rig.wrist_offset - arm.head).normalize_or_zero();
            if wanted != Vec3::ZERO && rest_direction != Vec3::ZERO {
                let turn = rig::align(rest_direction, root_rotation.inverse() * wanted);
                let most = FOREARM_TURN_MAX_DEG.to_radians();
                let angle = turn.to_axis_angle().1;
                let turn = if angle > most { Quat::IDENTITY.slerp(turn, most / angle) } else { turn };
                wrist_in_model = arm.head + turn * (rig.wrist_offset - arm.head);
                let (top, wrist) = arm.turned(turn, rig.wrist_rotation);
                if let Ok(mut transform) = transforms.get_mut(arm.top) {
                    transform.rotation = top;
                }
                if let Ok(mut transform) = transforms.get_mut(arm.wrist) {
                    transform.rotation = wrist;
                }
            }
        }

        let root_translation = wrist_world - root_rotation * (wrist_in_model * rig.scale);
        {
            let Ok(mut transform) = transforms.get_mut(root) else {
                continue;
            };
            transform.rotation = root_rotation;
            transform.scale = Vec3::splat(rig.scale);
            transform.translation = root_translation;
        }
        let wrist_rotation = root_rotation * rig.wrist_rotation;

        for digit in &rig.digits {
            pose_digit(digit, &posture, offset, wrist_world, wrist_rotation, rig.scale, &mut transforms);
        }
    }
}

fn pose_digit(
    digit: &Digit,
    posture: &on_hand::skeleton::Posture,
    offset: Vec3,
    wrist_world: Vec3,
    wrist_rotation: Quat,
    scale: f32,
    transforms: &mut Query<&mut Transform>,
) {
    let finger = digit.finger;
    let joints = [
        posture.joint(Joint::Base(finger)) + offset,
        posture.joint(Joint::Middle(finger)) + offset,
        posture.joint(Joint::Distal(finger)) + offset,
        posture.joint(Joint::Tip(finger)) + offset,
    ];
    let Some(first) = joints.len().checked_sub(digit.bones.len()) else {
        return;
    };
    let aims = &joints[first..];

    let mut parent_rotation = wrist_rotation;
    let mut parent_position = wrist_world;

    for (index, bone) in digit.bones.iter().enumerate() {
        let head = if index == 0 {
            parent_position + parent_rotation * (bone.rest_translation * scale)
        } else {
            aims[index - 1]
        };
        let aim = aims[index];

        let Ok(mut transform) = transforms.get_mut(bone.entity) else {
            continue;
        };
        let inverse_parent = parent_rotation.inverse();
        transform.translation = (inverse_parent * (head - parent_position)) / scale;

        let wanted = (aim - head).normalize_or_zero();
        if wanted != Vec3::ZERO {
            let local = inverse_parent * wanted;
            let rest_direction = bone.rest_rotation * Vec3::Y;
            transform.rotation = rig::align(rest_direction, local) * bone.rest_rotation;
        }

        parent_rotation *= transform.rotation;
        parent_position = head;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_the_forearm_leaves_the_hand_as_it_was() {
        let top_parent = Quat::from_euler(EulerRot::XYZ, 0.3, -1.1, 2.0);
        let top_rest = Quat::from_euler(EulerRot::XYZ, 0.9, 0.4, -2.6);
        let between = Quat::from_euler(EulerRot::XYZ, 0.02, 0.01, 0.03);
        let wrist_rest = Quat::from_euler(EulerRot::XYZ, 0.18, 0.01, -0.12);
        let wrist_in_model = top_parent * top_rest * between * wrist_rest;
        let arm = Forearm {
            top: Entity::PLACEHOLDER,
            top_rest,
            top_parent,
            head: Vec3::ZERO,
            wrist: Entity::PLACEHOLDER,
            wrist_rest,
        };
        let turn = Quat::from_rotation_z(0.4) * Quat::from_rotation_x(-0.2);
        let (top, wrist) = arm.turned(turn, wrist_in_model);
        let forearm = top_parent * top;
        assert!(
            forearm.angle_between(turn * top_parent * top_rest) < 2e-3,
            "the forearm turns by exactly the turn"
        );
        let hand = top_parent * top * between * wrist;
        assert!(hand.angle_between(wrist_in_model) < 2e-3, "the hand keeps the orientation it was given");
    }
}
