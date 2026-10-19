//! The keyboard and the falling notes: building their meshes, pressing keys, moving bars and
//! labelling fingers.

use super::*;

pub(crate) fn rebuild_scene(world: &mut World) {
    let doomed: Vec<Entity> = world
        .query_filtered::<Entity, Or<(With<KeyTop>, With<Backdrop>, With<NoteBar>, With<FingerLabel>, With<Flash>)>>()
        .iter(world)
        .collect();
    for entity in doomed {
        world.entity_mut(entity).despawn();
    }
    let _ = world.run_system_cached(setup_keyboard);
    let _ = world.run_system_cached(setup_notes);
    let _ = world.run_system_cached(setup_flashes);
    let _ = world.run_system_cached(place_camera);
}

/// Everything around the keys that `setup_keyboard` lays down with them, and so takes
/// away with them when the scene is rebuilt.
#[derive(Component)]
pub(crate) struct Backdrop;

#[derive(Component)]
pub(crate) struct KeyTop {
    pub(crate) midi: u8,
    pub(crate) rest: Vec3,
}

#[derive(Component)]
pub(crate) struct NoteBar {
    pub(crate) start: f64,
    pub(crate) duration: f64,
    pub(crate) hand: Hand,
    pub(crate) finger: Option<u8>,
}

#[derive(Component)]
pub(crate) struct FingerLabel;

pub(crate) fn setup_keyboard(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut lacquers: ResMut<Assets<Lacquered>>,
    mut images: ResMut<Assets<Image>>,
    performance: Res<Performance>,
) {
    let layout = &performance.layout;
    let white_texture = images.add(skin::white_key());
    let black_texture = images.add(skin::black_key());
    let (cx, cy) = layout.centre_mm();
    let tilt = CAMERA_TILT_DEG.to_radians();
    let eye = Vec3::new(cx, cy - LACQUER_EYE_MM * tilt.sin(), LACQUER_EYE_MM * tilt.cos());

    for midi in layout.keys() {
        let (x0, x1, y0, y1) = layout.key_rect(midi);
        let (width, length) = (x1 - x0, y1 - y0);
        let black = is_black(midi);

        let material = lacquers.add(key_material(
            if black { black_texture.clone() } else { white_texture.clone() },
            black,
            eye,
        ));

        let (mesh, z) = if black {
            (
                meshes.add(key_mesh(
                    Vec3::new(width, length, BLACK_KEY_HEIGHT),
                    BLACK_KEY_EDGE_MM,
                    BLACK_KEY_NARROWING_MM,
                    BLACK_KEY_RECESS_MM,
                )),
                BLACK_KEY_HEIGHT / 2.0,
            )
        } else {
            (
                meshes.add(key_mesh(
                    Vec3::new(width - KEY_GAP_MM, length, WHITE_KEY_DEPTH_MM),
                    WHITE_KEY_EDGE_MM,
                    0.0,
                    0.0,
                )),
                -WHITE_KEY_DEPTH_MM / 2.0,
            )
        };
        let rest = Vec3::new((x0 + x1) / 2.0, (y0 + y1) / 2.0, z);
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(rest),
            KeyTop { midi, rest },
        ));
    }

    let centre_x = (layout.left_mm() + layout.right_mm()) / 2.0;

    let octave = materials.add(painted(
        images.add(skin::octave_line()),
        glowing(Color::srgb(0.62, 0.72, 0.95), OCTAVE_LINE_GLOW),
        AlphaMode::Add,
    ));
    let line = meshes.add(Rectangle::new(OCTAVE_LINE_WIDTH_MM, layout.lane_height));
    for midi in layout.keys() {
        if midi % 12 != 0 {
            continue;
        }
        let (left, _, _, _) = layout.key_rect(midi);
        commands.spawn((
            Mesh3d(line.clone()),
            MeshMaterial3d(octave.clone()),
            Transform::from_xyz(left, layout.lane_height / 2.0, OCTAVE_LINE_Z),
            Backdrop,
        ));
    }

    let case_depth = -layout.bottom_mm() * 1.4;
    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(layout.width_mm(), case_depth))),
        MeshMaterial3d(materials.add(painted(images.add(skin::case()), Color::WHITE, AlphaMode::Opaque))),
        Transform::from_xyz(centre_x, -case_depth / 2.0, CASE_Z),
        Backdrop,
    ));

    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(layout.width_mm(), BACK_GLOW_DEPTH_MM))),
        MeshMaterial3d(materials.add(painted(
            images.add(skin::back_glow()),
            glowing(Color::srgb(0.0, 0.14, 1.0), BACK_GLOW_GLOW),
            AlphaMode::Add,
        ))),
        Transform::from_xyz(centre_x, BACK_GLOW_DEPTH_MM / 2.0, LANE_Z - 1.0),
        Backdrop,
    ));

    let hit = materials.add(painted(
        images.add(skin::hit_line()),
        glowing(Color::srgb(0.40, 1.0, 1.0), HIT_LINE_GLOW),
        AlphaMode::Add,
    ));
    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(layout.width_mm(), HIT_LINE_DEPTH_MM))),
        MeshMaterial3d(hit),
        Transform::from_xyz(centre_x, 0.0, LANE_Z - 1.0),
        Backdrop,
    ));
}

pub(crate) fn setup_notes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<NoteMaterial>>,
    performance: Res<Performance>,
    note_colours: Res<NoteColours>,
) {
    let layout = &performance.layout;
    let colours: Vec<Handle<NoteMaterial>> =
        note_colours.0.iter().map(|colour| materials.add(note_material(*colour))).collect();

    for note in &performance.timeline.notes {
        if !layout.shows(note.midi) {
            continue;
        }
        let (x0, x1, _, _) = layout.note_rect(note.midi, 0.0, 0.0);
        let width = (x1 - x0) * NOTE_WIDTH;
        let mesh = meshes.add(note_mesh(width, layout.note_height(note.end - note.start).max(6.0)));
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(colours[note.hand as usize].clone()),
            Transform::from_xyz((x0 + x1) / 2.0, 0.0, NOTE_Z),
            Visibility::Hidden,
            NoteBar {
                start: note.start,
                duration: note.end - note.start,
                hand: note.hand,
                finger: note.finger.map(|f| f.number()),
            },
        ));
    }
}

pub(crate) fn key_mesh(size: Vec3, radius: f32, narrowing: f32, recess: f32) -> Mesh {
    let half = size / 2.0;
    let inner = half - Vec3::splat(radius);
    let samples = |h: f32| -> Vec<f32> {
        let arc: Vec<f32> = (0..=KEY_EDGE_SEGMENTS)
            .map(|k| {
                let angle = std::f32::consts::FRAC_PI_4 * k as f32 / KEY_EDGE_SEGMENTS as f32;
                (h - radius) + radius * angle.tan()
            })
            .collect();
        arc.iter().rev().map(|t| -t).chain(arc.iter().copied()).collect()
    };
    let narrow = narrowing / size.x;
    let pull = recess / size.y;

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    let mut indices = Vec::new();
    for (normal, across, along) in [
        (Vec3::Z, Vec3::X, Vec3::Y),
        (Vec3::NEG_Z, Vec3::Y, Vec3::X),
        (Vec3::X, Vec3::Y, Vec3::Z),
        (Vec3::NEG_X, Vec3::Z, Vec3::Y),
        (Vec3::Y, Vec3::Z, Vec3::X),
        (Vec3::NEG_Y, Vec3::X, Vec3::Z),
    ] {
        let (us, vs) = (samples(half.dot(across)), samples(half.dot(along)));
        let base = positions.len() as u32;
        for &v in &vs {
            for &u in &us {
                let flat = normal * half + across * u + along * v;
                let core = flat.clamp(-inner, inner);
                let outward = (flat - core).normalize();
                let p = core + outward * radius;
                let rise = (p.z + half.z) / size.z;
                let jacobian = Mat3::from_cols(
                    Vec3::new(1.0 - narrow * rise, 0.0, 0.0),
                    Vec3::new(0.0, 1.0 - pull * rise, 0.0),
                    Vec3::new(-p.x * narrow / size.z, (half.y - p.y) * pull / size.z, 1.0),
                );
                positions.push([p.x * (1.0 - narrow * rise), p.y + (half.y - p.y) * pull * rise, p.z]);
                normals.push((jacobian.inverse().transpose() * outward).normalize().to_array());
                uvs.push([(p.x + half.x) / size.x, (half.y - p.y) / size.y]);
            }
        }
        let row = us.len() as u32;
        for j in 0..vs.len() as u32 - 1 {
            for i in 0..row - 1 {
                let a = base + j * row + i;
                indices.extend([a, a + 1, a + row + 1, a, a + row + 1, a + row]);
            }
        }
    }

    Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(bevy::mesh::Indices::U32(indices))
}

pub(crate) fn note_mesh(width: f32, height: f32) -> Mesh {
    let half = [width / 2.0, height / 2.0];
    let corners = [[-1.0, 1.0], [1.0, 1.0], [-1.0, -1.0], [1.0, -1.0]];
    let positions: Vec<[f32; 3]> = corners.iter().map(|[x, y]| [x * half[0], y * half[1], 0.0]).collect();
    let local: Vec<[f32; 2]> = positions.iter().map(|p| [p[0], p[1]]).collect();
    Mesh::new(
        bevy::mesh::PrimitiveTopology::TriangleList,
        RenderAssetUsages::RENDER_WORLD | RenderAssetUsages::MAIN_WORLD,
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4])
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, local)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, vec![half; 4])
    .with_inserted_indices(bevy::mesh::Indices::U32(vec![0, 2, 3, 0, 3, 1]))
}

pub(crate) fn bar_extent(layout: &Layout, bar: &NoteBar, now: f64) -> (f32, f32) {
    let bottom = layout.note_y(bar.start - now);
    let height = layout.note_height(bar.duration).max(6.0);
    (bottom, height)
}

/// A bar starts moving this long before it can enter the lane and keeps moving this long after
/// it has left. The renderer takes a mesh's previous position, for the motion vectors of the
/// temporal anti-aliasing, from the last time it moved; so a bar must already be moving the
/// frame before it shows, as it was when every bar moved every frame.
const BAR_LEAD_SECONDS: f64 = 0.5;

/// The bars that may be in the lane at a moment, so that the systems drawing them visit those
/// and not every note of the piece. The bars are listed in the order the query gave them when
/// the window was built, again whenever bars were added or changed shape.
#[derive(Default)]
pub(crate) struct BarWindow {
    bars: Vec<Entity>,
    index: crate::timeline::janela::TimeIndex,
}

/// Bars spawned, or given a component by the engine (which moves them in its storage and can
/// change the order a query lists them in), since a system last looked.
pub(crate) type ReshapedBars<'w, 's> = Query<
    'w,
    's,
    (),
    (
        With<NoteBar>,
        Or<(Added<NoteBar>, Added<bevy::camera::primitives::Aabb>, Added<bevy::pbr::PreviousGlobalTransform>)>,
    ),
>;

impl BarWindow {
    fn build<'a>(layout: &Layout, bars: impl Iterator<Item = (Entity, &'a NoteBar)>) -> Self {
        let lane = f64::from(layout.lane_height).max(1e-3);
        // A bar is in the lane from `lookahead` before its start until its drawn height (at
        // least 6 mm) has passed below the line.
        let (bars, spans): (Vec<Entity>, Vec<(f64, f64)>) = bars
            .map(|(entity, bar)| {
                let shown_for = bar.duration.max(6.0 * layout.lookahead / lane);
                let span = (bar.start - layout.lookahead - BAR_LEAD_SECONDS, bar.start + shown_for + BAR_LEAD_SECONDS);
                (entity, span)
            })
            .unzip();
        Self { bars, index: crate::timeline::janela::TimeIndex::new(spans) }
    }

    fn at(&self, now: f64) -> impl Iterator<Item = Entity> + '_ {
        self.index.at(now).iter().map(|&i| self.bars[i as usize])
    }
}

pub(crate) fn move_notes(
    transport: Res<Transport>,
    performance: Res<Performance>,
    reshaped: ReshapedBars,
    mut window: Local<BarWindow>,
    mut shown: Local<Vec<Entity>>,
    mut bars: Query<(Entity, &NoteBar, &mut Transform, &mut Visibility)>,
) {
    medir!("move_notes");
    let layout = &performance.layout;
    let now = transport.position;
    let place = |bar: &NoteBar| {
        let (bottom, height) = bar_extent(layout, bar, now);
        let top = bottom + height;
        let y = bottom + height / 2.0 + (LANE_Z + 1.0 - NOTE_Z) * CAMERA_TILT_DEG.to_radians().tan();
        (y, top > 0.0 && bottom < layout.lane_height)
    };
    #[cfg(feature = "perfil")]
    if crate::perfil::sem_janela() {
        let mut visiveis = 0u64;
        for (_, bar, mut transform, mut visibility) in &mut bars {
            let (y, visible) = place(bar);
            transform.translation.y = y;
            *visibility = if visible { Visibility::Visible } else { Visibility::Hidden };
            visiveis += u64::from(visible);
        }
        crate::perfil::pico("barras_visiveis", visiveis);
        return;
    }

    if performance.is_changed() || !reshaped.is_empty() {
        *window = BarWindow::build(layout, bars.iter().map(|(entity, bar, _, _)| (entity, bar)));
    }
    let mut still = Vec::with_capacity(shown.len() + 16);
    for entity in window.at(now) {
        let Ok((_, bar, mut transform, mut visibility)) = bars.get_mut(entity) else { continue };
        let (y, visible) = place(bar);
        transform.translation.y = y;
        visibility.set_if_neq(if visible { Visibility::Visible } else { Visibility::Hidden });
        if visible {
            still.push(entity);
        }
    }
    // Bars that were showing and are no longer in the window (after a jump in time).
    for entity in shown.drain(..) {
        if !still.contains(&entity) {
            if let Ok((_, _, _, mut visibility)) = bars.get_mut(entity) {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    *shown = still;
    #[cfg(feature = "perfil")]
    {
        crate::perfil::pico("barras_visiveis", shown.len() as u64);
        if crate::perfil::conferir_janela() {
            for (entity, bar, transform, visibility) in &bars {
                let (y, visible) = place(bar);
                let wanted = if visible { Visibility::Visible } else { Visibility::Hidden };
                if *visibility != wanted || (visible && transform.translation.y != y) {
                    crate::perfil::divergencia("move_notes", || {
                        format!("t={now} {entity}: {visibility:?} y={} != {wanted:?} y={y}", transform.translation.y)
                    });
                }
            }
        }
    }
}

pub(crate) fn press_keys(
    note_colours: Res<NoteColours>,
    down: Res<Keys>,
    mut materials: ResMut<Assets<Lacquered>>,
    keys: Query<(&KeyTop, &mut Transform, &MeshMaterial3d<Lacquered>)>,
) {
    medir!("press_keys");
    let states = &down.0;
    for (key, mut transform, material) in keys {
        let depth = states.depth_of(key.midi);
        let black = is_black(key.midi);
        let dip = if black { BLACK_KEY_DIP } else { KEY_DIP };
        let at = key.rest - Vec3::new(0.0, 0.0, depth * dip);
        // Written only when they change: touching a transform or a material marks it
        // changed, and a changed material is uploaded to the GPU again.
        if transform.translation != at {
            transform.translation = at;
        }

        let (base, emissive) = match states.hand_on(key.midi) {
            Some(hand) => {
                let tint = note_colours.0[hand as usize];
                let glow = if black { BLACK_KEY_GLOW } else { KEY_GLOW };
                (mix(Color::WHITE, tint, depth), tint.to_linear() * (depth * glow))
            }
            None => (Color::WHITE, LinearRgba::BLACK),
        };
        let unchanged = materials
            .get(&material.0)
            .is_some_and(|current| current.base.base_color == base && current.base.emissive == emissive);
        if !unchanged {
            if let Some(mut current) = materials.get_mut(&material.0) {
                current.base.base_color = base;
                current.base.emissive = emissive;
            }
        }
    }
}

pub(crate) fn update_labels(
    mut commands: Commands,
    transport: Res<Transport>,
    performance: Res<Performance>,
    camera: Query<(&Camera, &GlobalTransform)>,
    bars: Query<(Entity, &NoteBar, &Transform)>,
    reshaped: ReshapedBars,
    mut window: Local<BarWindow>,
    labels: Query<(&mut Node, &mut Text, &mut TextColor), With<FingerLabel>>,
) {
    medir!("update_labels");
    let Ok((camera, camera_transform)) = camera.single() else {
        return;
    };
    let inset = camera.logical_viewport_rect().map(|rect| rect.min).unwrap_or(Vec2::ZERO);
    let layout = &performance.layout;
    let now = transport.position;

    let wanted_of = |bar: &NoteBar, transform: &Transform, wanted: &mut Vec<(f32, u8, Hand, Vec3)>| {
        let Some(finger) = bar.finger else { return };
        let (bottom, height) = bar_extent(layout, bar, now);
        let top = bottom + height;
        let visible_bottom = bottom.max(0.0);
        let visible_height = top - visible_bottom;
        if visible_height < MIN_LABELLED_HEIGHT_MM || bottom > layout.lane_height {
            return;
        }
        let anchor = Vec3::new(
            transform.translation.x,
            visible_bottom + (visible_height / 2.0).min(13.0),
            LANE_Z + 1.0,
        );
        wanted.push((visible_bottom, finger, bar.hand, anchor));
    };
    let every_bar = |wanted: &mut Vec<(f32, u8, Hand, Vec3)>| {
        for (_, bar, transform) in &bars {
            wanted_of(bar, transform, wanted);
        }
    };
    let mut wanted: Vec<(f32, u8, Hand, Vec3)> = Vec::new();
    #[cfg(feature = "perfil")]
    let todas = crate::perfil::sem_janela();
    #[cfg(not(feature = "perfil"))]
    let todas = false;
    if todas {
        every_bar(&mut wanted);
    } else {
        // Only the bars that may be in the lane, in the order the query lists them, so that
        // labels at the same height sort as they did over every bar.
        if performance.is_changed() || !reshaped.is_empty() {
            *window = BarWindow::build(layout, bars.iter().map(|(entity, bar, _)| (entity, bar)));
        }
        for entity in window.at(now) {
            if let Ok((_, bar, transform)) = bars.get(entity) {
                wanted_of(bar, transform, &mut wanted);
            }
        }
    }
    wanted.sort_by(|a, b| a.0.total_cmp(&b.0));
    #[cfg(feature = "perfil")]
    if crate::perfil::conferir_janela() && !todas {
        let mut all = Vec::new();
        every_bar(&mut all);
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        if all != wanted {
            crate::perfil::divergencia("update_labels", || format!("t={now}: {} != {} rótulos", wanted.len(), all.len()));
        }
    }
    wanted.truncate(MAX_LABELS);

    let mut slot = 0;
    for (mut node, mut text, mut colour) in labels {
        match wanted.get(slot) {
            Some((_, finger, hand, anchor)) => match camera.world_to_viewport(camera_transform, *anchor) {
                Ok(screen) => {
                    let screen = screen - inset;
                    node.display = Display::Flex;
                    node.left = Val::Px(screen.x - LABEL_FONT_PX * 0.28);
                    node.top = Val::Px(screen.y - LABEL_FONT_PX * 0.62);
                    let digit = finger.to_string();
                    if text.0 != digit {
                        text.0 = digit;
                    }
                    colour.0 = match hand {
                        Hand::Left => Color::srgb(0.03, 0.11, 0.22),
                        Hand::Right => Color::srgb(0.20, 0.08, 0.01),
                    };
                }
                Err(_) => node.display = Display::None,
            },
            None => node.display = Display::None,
        }
        slot += 1;
    }

    for _ in slot..wanted.len().min(slot + 16) {
        commands.spawn((
            Text::new(""),
            TextFont { font_size: bevy::text::FontSize::Px(LABEL_FONT_PX), ..default() },
            TextColor(Color::WHITE),
            Node { position_type: PositionType::Absolute, display: Display::None, ..default() },
            FingerLabel,
        ));
    }
}
