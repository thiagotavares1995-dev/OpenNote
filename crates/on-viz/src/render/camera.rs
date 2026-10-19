//! The tilted orthographic camera, its lights, and fitting the view between the menu and
//! transport bars.

use super::*;

pub(crate) fn setup_camera(mut commands: Commands, performance: Res<Performance>) {
    let layout = &performance.layout;
    let (cx, cy) = layout.centre_mm();

    commands.spawn((
        Camera3d::default(),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        bevy::post_process::effect_stack::Vignette {
            intensity: VIGNETTE,
            radius: 0.95,
            smoothness: 2.2,
            ..default()
        },
        camera_projection(layout),
        camera_placement(layout),
        bevy::post_process::bloom::Bloom {
            intensity: BLOOM_INTENSITY,
            prefilter: bevy::post_process::bloom::BloomPrefilter { threshold: 1.0, threshold_softness: 0.35 },
            composite_mode: bevy::post_process::bloom::BloomCompositeMode::Additive,
            low_frequency_boost: BLOOM_HAZE,
            max_mip_dimension: 512,
            ..bevy::post_process::bloom::Bloom::NATURAL
        },
        bevy::pbr::ScreenSpaceAmbientOcclusion::default(),
        bevy::anti_alias::taa::TemporalAntiAliasing::default(),
        Msaa::Off,
    ));

    commands.spawn((
        DirectionalLight { illuminance: 4_650.0, shadow_maps_enabled: true, ..default() },
        KeyLight,
        Transform::from_xyz(cx + 380.0, cy - 300.0, 1_050.0).looking_at(Vec3::new(cx, cy, 0.0), Vec3::Z),
        bevy::light::CascadeShadowConfigBuilder {
            num_cascades: 2,
            first_cascade_far_bound: 900.0,
            maximum_distance: 2_400.0,
            ..default()
        }
        .build(),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 1_250.0,
            color: Color::srgb(0.86, 0.88, 1.0),
            shadow_maps_enabled: false,
            ..default()
        },
        Transform::from_xyz(cx, cy - 1_100.0, 170.0).looking_at(Vec3::new(cx, cy, 30.0), Vec3::Z),
    ));
    commands.insert_resource(bevy::light::DirectionalLightShadowMap { size: 4096 });
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.52, 0.62, 0.90),
        brightness: 22.0,
        ..default()
    });
    commands.insert_resource(ClearColor(Color::BLACK));
}

pub(crate) fn camera_placement(layout: &Layout) -> Transform {
    let (cx, cy) = layout.centre_mm();
    let tilt = CAMERA_TILT_DEG.to_radians();
    Transform::from_xyz(cx, cy - CAMERA_HEIGHT_MM * tilt.sin(), CAMERA_HEIGHT_MM * tilt.cos())
        .looking_at(Vec3::new(cx, cy, 0.0), Vec3::Y)
}

pub(crate) fn camera_projection(layout: &Layout) -> Projection {
    Projection::Orthographic(OrthographicProjection {
        scaling_mode: bevy::camera::ScalingMode::AutoMin {
            min_width: layout.width_mm() * VIEW_MARGIN,
            min_height: layout.height_mm() * VIEW_MARGIN,
        },
        near: 0.0,
        far: CAMERA_HEIGHT_MM * 4.0,
        ..OrthographicProjection::default_3d()
    })
}

pub(crate) fn place_camera(
    performance: Res<Performance>,
    cameras: Query<(&mut Transform, &mut Projection), With<Camera3d>>,
) {
    let layout = &performance.layout;
    for (mut transform, mut projection) in cameras {
        *transform = camera_placement(layout);
        *projection = camera_projection(layout);
    }
}

pub(crate) fn fit_camera_viewport(
    inset: Res<ViewportInset>,
    windows: Query<&Window>,
    cameras: Query<&mut Camera, With<Camera3d>>,
) {
    medir!("fit_camera_viewport");
    if inset.top <= 0.5 && inset.bottom <= 0.5 {
        for mut camera in cameras {
            if camera.viewport.is_some() {
                camera.viewport = None;
            }
        }
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    let scale = window.scale_factor();
    let top = (inset.top * scale).round() as u32;
    let bottom = (inset.bottom * scale).round() as u32;
    let size = window.physical_size();
    if size.x == 0 || size.y <= top + bottom {
        return;
    }

    let wanted = bevy::camera::Viewport {
        physical_position: UVec2::new(0, top),
        physical_size: UVec2::new(size.x, size.y - top - bottom),
        ..default()
    };
    for mut camera in cameras {
        let unchanged = camera.viewport.as_ref().is_some_and(|current| {
            current.physical_position == wanted.physical_position
                && current.physical_size == wanted.physical_size
        });
        if !unchanged {
            camera.viewport = Some(wanted.clone());
        }
    }
}

/// The light that casts the hands' shadows.
#[derive(Component)]
pub(crate) struct KeyLight;

/// How much the viewer draws. Effects are the sparks, flashes and bloom, which some find
/// too busy; low quality drops the screen-space ambient occlusion, temporal anti-aliasing
/// and shadows that a modest graphics card struggles with.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visuals {
    pub effects: bool,
    pub high_quality: bool,
}

impl Default for Visuals {
    fn default() -> Self {
        Self { effects: true, high_quality: true }
    }
}

pub(crate) fn apply_visuals(
    mut commands: Commands,
    visuals: Res<Visuals>,
    cameras: Query<(Entity, &mut bevy::post_process::bloom::Bloom), With<Camera3d>>,
    mut lights: Query<&mut DirectionalLight, With<KeyLight>>,
) {
    medir!("apply_visuals");
    if !visuals.is_changed() {
        return;
    }
    for (camera, mut bloom) in cameras {
        bloom.intensity = if visuals.effects { BLOOM_INTENSITY } else { 0.0 };
        let mut camera = commands.entity(camera);
        if visuals.high_quality {
            camera.insert((
                bevy::pbr::ScreenSpaceAmbientOcclusion::default(),
                bevy::anti_alias::taa::TemporalAntiAliasing::default(),
                Msaa::Off,
            ));
        } else {
            camera
                .remove::<(bevy::pbr::ScreenSpaceAmbientOcclusion, bevy::anti_alias::taa::TemporalAntiAliasing)>()
                .insert(Msaa::Sample4);
        }
    }
    for mut light in &mut lights {
        light.shadow_maps_enabled = visuals.high_quality;
    }
}
