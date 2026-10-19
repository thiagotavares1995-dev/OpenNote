//! The effects that follow struck keys: sparks, flashes, light beams and the lights that tint
//! what is near.

use super::*;

#[derive(Component)]
pub(crate) struct Spark;

#[derive(Component)]
pub(crate) struct NoteLight;

#[derive(Component)]
pub(crate) struct Flash {
    pub(crate) midi: u8,
    pub(crate) spread: f32,
    pub(crate) beam: bool,
}

#[derive(Resource)]
pub(crate) struct SparkColours {
    pub(crate) hot: [Handle<StandardMaterial>; 2],
    pub(crate) cool: [Handle<StandardMaterial>; 2],
    pub(crate) texture: Handle<Image>,
}

impl SparkColours {
    fn new(materials: &mut Assets<StandardMaterial>, texture: Handle<Image>, colours: [Color; 2]) -> Self {
        let mut dust = |colour: Color, whiteness: f32, glow: f32| {
            materials.add(StandardMaterial {
                base_color: glowing(mix(colour, Color::WHITE, whiteness), glow),
                base_color_texture: Some(texture.clone()),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                ..default()
            })
        };
        Self {
            hot: colours.map(|colour| dust(colour, 0.70, SPARK_GLOW * 2.6)),
            cool: colours.map(|colour| dust(colour, 0.06, SPARK_GLOW)),
            texture,
        }
    }
}

/// The sparks' materials are made from the hand colours, so they are made again when those
/// change; everything else reads the colours as it draws or is rebuilt with the scene.
pub(crate) fn recolour_sparks(
    note_colours: Res<NoteColours>,
    mut sparks: ResMut<SparkColours>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    medir!("recolour_sparks");
    if note_colours.is_changed() && !note_colours.is_added() {
        *sparks = SparkColours::new(&mut materials, sparks.texture.clone(), note_colours.0);
    }
}

pub(crate) fn setup_sparks(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    note_colours: Res<NoteColours>,
) {
    let texture = images.add(skin::spark());
    let mesh = meshes.add(Rectangle::new(1.0, 1.0));
    let colours = SparkColours::new(&mut materials, texture, note_colours.0);

    for _ in 0..MAX_SPARKS {
        commands.spawn((
            Mesh3d(mesh.clone()),
            MeshMaterial3d(colours.cool[0].clone()),
            Transform::from_xyz(0.0, 0.0, LANE_Z + 2.0),
            Visibility::Hidden,
            Spark,
        ));
    }
    commands.insert_resource(colours);
}

pub(crate) fn setup_flashes(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    performance: Res<Performance>,
    note_colours: Res<NoteColours>,
) {
    let layout = &performance.layout;
    let mut glow = |texture: Handle<Image>, whiteness: f32, brightness: f32| {
        note_colours.0.map(|colour| {
            materials.add(StandardMaterial {
                base_color: glowing(mix(colour, Color::WHITE, whiteness), brightness),
                base_color_texture: Some(texture.clone()),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                ..default()
            })
        })
    };
    let colours =
        [glow(images.add(skin::flash()), 0.20, FLASH_GLOW), glow(images.add(skin::beam()), 0.05, BEAM_GLOW)];

    let quad = meshes.add(Rectangle::new(FLASH_SIZE_MM * skin::FLASH_STREAK, FLASH_SIZE_MM));
    let shaft = meshes.add(
        Mesh::from(Rectangle::new(BEAM_WIDTH_MM, BEAM_HEIGHT_MM))
            .translated_by(Vec3::Y * BEAM_HEIGHT_MM / 2.0),
    );
    for midi in layout.keys() {
        let (x0, x1, _, _) = layout.key_rect(midi);
        let spread = (x1 - x0) / on_hand::keyboard::WHITE_KEY_WIDTH;
        let x = (x0 + x1) / 2.0;
        commands.spawn((
            Mesh3d(quad.clone()),
            MeshMaterial3d(colours[0][0].clone()),
            Transform::from_xyz(x, FLASH_HEIGHT_MM, LANE_Z + 1.0),
            Visibility::Hidden,
            Flash { midi, spread, beam: false },
        ));
        commands.spawn((
            Mesh3d(shaft.clone()),
            MeshMaterial3d(colours[1][0].clone()),
            Transform::from_xyz(x, 0.0, LANE_Z + 0.5),
            Visibility::Hidden,
            Flash { midi, spread, beam: true },
        ));
    }
    commands.insert_resource(FlashColours(colours));
}

#[derive(Resource)]
pub(crate) struct FlashColours(pub(crate) [[Handle<StandardMaterial>; 2]; 2]);

pub(crate) fn raise_flashes(
    colours: Res<FlashColours>,
    visuals: Res<Visuals>,
    keys: Res<Keys>,
    flashes: Query<(&Flash, &mut Transform, &mut Visibility, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    medir!("raise_flashes");
    if !visuals.effects {
        if visuals.is_changed() {
            for (_, _, mut visibility, _) in flashes {
                *visibility = Visibility::Hidden;
            }
        }
        return;
    }
    let states = &keys.0;
    for (flash, mut transform, mut visibility, mut material) in flashes {
        let depth = states.depth_of(flash.midi);
        let Some(hand) = states.hand_on(flash.midi).filter(|_| depth > 0.0) else {
            if *visibility != Visibility::Hidden {
                *visibility = Visibility::Hidden;
            }
            continue;
        };
        transform.scale = if flash.beam {
            Vec3::new(flash.spread, depth.powf(0.8), 1.0)
        } else {
            Vec3::splat(flash.spread * depth.powf(0.55))
        };
        *visibility = Visibility::Visible;

        let wanted = &colours.0[flash.beam as usize][hand as usize];
        if material.0.id() != wanted.id() {
            material.0 = wanted.clone();
        }
    }
}

pub(crate) fn setup_note_lights(mut commands: Commands) {
    for _ in 0..MAX_NOTE_LIGHTS {
        commands.spawn((
            PointLight {
                intensity: 0.0,
                range: NOTE_LIGHT_RANGE_MM,
                radius: 12.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_xyz(0.0, 0.0, NOTE_LIGHT_HEIGHT_MM),
            Visibility::Hidden,
            NoteLight,
        ));
    }
}

pub(crate) fn light_struck_keys(
    performance: Res<Performance>,
    note_colours: Res<NoteColours>,
    keys: Res<Keys>,
    lights: Query<(&mut PointLight, &mut Transform, &mut Visibility), With<NoteLight>>,
) {
    medir!("light_struck_keys");
    let layout = &performance.layout;
    let states = &keys.0;
    let offset = Vec3::Y * KEYBOARD_Y_MM;

    let mut sounding: Vec<(f32, u8, Hand)> = layout
        .keys()
        .filter_map(|midi| {
            let depth = states.depth_of(midi);
            let hand = states.hand_on(midi)?;
            (depth > 0.02).then_some((depth, midi, hand))
        })
        .collect();
    sounding.sort_by(|a, b| b.0.total_cmp(&a.0));

    for (slot, (mut light, mut transform, mut visibility)) in lights.into_iter().enumerate() {
        match sounding.get(slot) {
            Some((depth, midi, hand)) => {
                let (x0, x1, y0, y1) = layout.key_rect(*midi);
                light.color = note_colours.0[*hand as usize];
                light.intensity = NOTE_LIGHT_LUMENS * depth;
                transform.translation =
                    Vec3::new((x0 + x1) / 2.0, y0 + (y1 - y0) * 0.28, NOTE_LIGHT_HEIGHT_MM) + offset;
                *visibility = Visibility::Visible;
            }
            None => {
                if *visibility != Visibility::Hidden {
                    *visibility = Visibility::Hidden;
                    light.intensity = 0.0;
                }
            }
        }
    }
}

pub(crate) fn update_sparks(
    transport: Res<Transport>,
    performance: Res<Performance>,
    colours: Res<SparkColours>,
    visuals: Res<Visuals>,
    playing: Res<Playing>,
    sparks: Query<(&mut Transform, &mut Visibility, &mut MeshMaterial3d<StandardMaterial>), With<Spark>>,
) {
    medir!("update_sparks");
    if !visuals.effects {
        if visuals.is_changed() {
            for (_, mut visibility, _) in sparks {
                *visibility = Visibility::Hidden;
            }
        }
        return;
    }
    let layout = &performance.layout;
    let now = transport.position;
    let mut slots = sparks.into_iter();
    let stagger = SPARK_LIFE / SPARKS_PER_NOTE as f32;

    for (index, note) in performance.timeline.notes.iter().enumerate() {
        if !playing.0[note.hand as usize] {
            continue;
        }
        let done = (note.end + f64::from(SPARK_TAIL_SECONDS)).min(note.restruck.unwrap_or(f64::INFINITY));
        if now < note.start || now >= done {
            continue;
        }
        let elapsed = (now - note.start) as f32;
        let throwing = (note.end.min(done) - note.start) as f32;

        let evaporation =
            (((done - now) as f32) / SPARK_TAIL_SECONDS).clamp(0.0, 1.0).powf(SPARK_EVAPORATE_SHAPE);

        let (x0, x1, _, _) = layout.key_rect(note.midi);
        let centre = (x0 + x1) / 2.0;
        let width = x1 - x0;

        for spark in 0..SPARKS_PER_NOTE {
            let Some((mut transform, mut visibility, mut material)) = slots.next() else {
                return;
            };
            *visibility = Visibility::Hidden;

            let since = elapsed - spark as f32 * stagger;
            if since < 0.0 {
                continue;
            }
            let turn = (since / SPARK_LIFE).floor();
            let age = since - turn * SPARK_LIFE;
            let seed = (index as u32)
                .wrapping_mul(0x9e37_79b9)
                .wrapping_add((spark as u32).wrapping_mul(0x85eb_ca6b))
                ^ (turn as u32).wrapping_mul(0xc2b2_ae35);

            let born = since - age;
            if born > throwing {
                continue;
            }
            if born > SPARK_EMIT_SECONDS && scramble(seed ^ 0x7f4a_7c15) > SPARK_SUSTAIN_SHARE {
                continue;
            }

            let strand = seed.wrapping_div(SPARK_FILAMENT).wrapping_mul(0x9e37_79b9)
                ^ (turn as u32).wrapping_mul(0x85eb_ca6b);

            let reach = width * SPARK_WANDER_KEYS * (2.0 * scramble(strand ^ 0x94d0_49bb) - 1.0);
            let upward = SPARK_RISE_MM * (0.40 + 0.60 * scramble(strand ^ 0x5bf0_3635).powf(1.7));
            let size = SPARK_SIZE_MM.start
                + (SPARK_SIZE_MM.end - SPARK_SIZE_MM.start) * scramble(seed ^ 0x27d4_eb2f);
            let phase = scramble(seed ^ 0x1656_67b1) * std::f32::consts::TAU;

            let lean = if note.id.0 % 2 == 0 { 1.0 } else { -1.0 };
            let place = |age: f32| {
                let height = upward * age - 0.5 * SPARK_GRAVITY_MM * age * age;
                let opened = (age / SPARK_LIFE).powf(SPARK_WANDER_SHAPE);
                let spread = reach * (SPARK_WANDER_AT_BIRTH + (1.0 - SPARK_WANDER_AT_BIRTH) * opened);
                let curl = (phase + age * SPARK_CURL_RATE).sin() * width * SPARK_CURL_KEYS * age;
                let drift = lean * width * SPARK_DRIFT_KEYS * age * age;
                Vec2::new(centre + spread + curl + drift, height)
            };
            let here = place(age);
            if here.y < 0.0 {
                continue;
            }

            let taper = (1.0 - age / SPARK_LIFE).clamp(0.0, 1.0).powf(0.45);
            let scale = size * taper * evaporation;
            if scale <= 0.0 {
                continue;
            }
            let trail = here - place((age - SPARK_TRAIL_SECONDS).max(0.0));

            transform.translation = here.extend(LANE_Z + 2.0);
            transform.rotation = Quat::from_rotation_z(f32::atan2(-trail.x, trail.y));
            transform.scale = Vec3::new(scale, scale + trail.length() * taper, 1.0);
            *visibility = Visibility::Visible;

            let hand = note.hand as usize;
            let wanted = if age < SPARK_WHITE_FOR { &colours.hot[hand] } else { &colours.cool[hand] };
            if material.0.id() != wanted.id() {
                material.0 = wanted.clone();
            }
        }
    }

    for (_, mut visibility, _) in slots {
        if *visibility != Visibility::Hidden {
            *visibility = Visibility::Hidden;
        }
    }
}

pub(crate) fn scramble(mut x: u32) -> f32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}
