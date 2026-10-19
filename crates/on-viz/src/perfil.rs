//! Measurement for the export, built only with the `perfil` feature: when `OPENNOTE_PERFIL`
//! names a file, the export writes there how long each phase, schedule and system took, how
//! many app updates it ran per frame and a few peaks. `OPENNOTE_PARAR_EM` stops after that
//! many frames and `OPENNOTE_HASH_QUADROS` writes a hash of every raw frame, one per line.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

#[derive(Default)]
struct Sistema {
    vezes: u64,
    total_ns: u128,
    max_ns: u128,
}

#[derive(Default)]
struct Dados {
    marcas: Vec<(String, f64)>,
    sistemas: BTreeMap<&'static str, Sistema>,
    contadores: BTreeMap<&'static str, u64>,
    picos: BTreeMap<&'static str, u64>,
}

fn inicio() -> Instant {
    static INICIO: OnceLock<Instant> = OnceLock::new();
    *INICIO.get_or_init(Instant::now)
}

fn dados() -> std::sync::MutexGuard<'static, Dados> {
    static DADOS: OnceLock<Mutex<Dados>> = OnceLock::new();
    DADOS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Seconds since the first mark, for the phases of the export.
pub fn marca(nome: &str) {
    let agora = inicio().elapsed().as_secs_f64();
    let mut d = dados();
    if !d.marcas.iter().any(|(n, _)| n == nome) {
        d.marcas.push((nome.to_string(), agora));
    }
}

pub fn contar(nome: &'static str, quanto: u64) {
    *dados().contadores.entry(nome).or_default() += quanto;
}

pub fn pico(nome: &'static str, valor: u64) {
    let mut d = dados();
    let p = d.picos.entry(nome).or_default();
    *p = (*p).max(valor);
}

pub fn somar(nome: &'static str, ns: u128) {
    let mut d = dados();
    let s = d.sistemas.entry(nome).or_default();
    s.vezes += 1;
    s.total_ns += ns;
    s.max_ns = s.max_ns.max(ns);
}

pub struct Medida(&'static str, Instant);

impl Drop for Medida {
    fn drop(&mut self) {
        somar(self.0, self.1.elapsed().as_nanos());
    }
}

pub fn medir(nome: &'static str) -> Medida {
    Medida(nome, Instant::now())
}

pub fn parar_em() -> Option<u32> {
    std::env::var("OPENNOTE_PARAR_EM").ok()?.parse().ok()
}

pub fn arquivo_hashes() -> Option<std::path::PathBuf> {
    std::env::var_os("OPENNOTE_HASH_QUADROS").map(Into::into)
}

/// A quick 64-bit hash of a frame (FNV-1a over 8-byte words); only to tell frames apart.
pub fn hash_quadro(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut chunks = bytes.chunks_exact(8);
    for c in &mut chunks {
        h ^= u64::from_le_bytes(c.try_into().unwrap());
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for &b in chunks.remainder() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

pub fn gravar() {
    let Some(caminho) = std::env::var_os("OPENNOTE_PERFIL") else { return };
    let d = dados();
    let marcas: serde_json::Map<String, serde_json::Value> =
        d.marcas.iter().map(|(n, t)| (n.clone(), serde_json::json!(t))).collect();
    let sistemas: serde_json::Map<String, serde_json::Value> = d
        .sistemas
        .iter()
        .map(|(n, s)| {
            (
                n.to_string(),
                serde_json::json!({
                    "vezes": s.vezes,
                    "total_s": s.total_ns as f64 / 1e9,
                    "media_us": if s.vezes > 0 { s.total_ns as f64 / s.vezes as f64 / 1e3 } else { 0.0 },
                    "max_us": s.max_ns as f64 / 1e3,
                }),
            )
        })
        .collect();
    let json = serde_json::json!({
        "marcas_s": marcas,
        "sistemas": sistemas,
        "contadores": d.contadores,
        "picos": d.picos,
    });
    if let Err(erro) = std::fs::write(&caminho, serde_json::to_string_pretty(&json).unwrap_or_default()) {
        eprintln!("perfil: não gravei {}: {erro}", std::path::Path::new(&caminho).display());
    }
}

#[cfg(feature = "render")]
pub(crate) mod agenda {
    //! Timestamps between the main schedules, to split each app update into its parts.
    use bevy::app::MainScheduleOrder;
    use bevy::ecs::schedule::ScheduleLabel;
    use bevy::prelude::*;
    use std::time::Instant;

    #[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
    struct AntesUpdate;
    #[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
    struct DepoisUpdate;
    #[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
    struct DepoisPostUpdate;

    #[derive(Resource, Default)]
    struct Relogio {
        first: Option<Instant>,
        antes_update: Option<Instant>,
        depois_update: Option<Instant>,
        depois_post: Option<Instant>,
    }

    pub(crate) fn instalar(app: &mut App) {
        app.init_resource::<Relogio>();
        for rotulo in [AntesUpdate.intern(), DepoisUpdate.intern(), DepoisPostUpdate.intern()] {
            app.init_schedule(rotulo);
        }
        {
            let mut ordem = app.world_mut().resource_mut::<MainScheduleOrder>();
            ordem.insert_before(Update, AntesUpdate);
            ordem.insert_after(Update, DepoisUpdate);
            ordem.insert_after(PostUpdate, DepoisPostUpdate);
        }
        app.add_systems(First, |mut r: ResMut<Relogio>| {
            let agora = Instant::now();
            if let (Some(f), Some(depois_post)) = (r.first, r.depois_post) {
                // From the end of PostUpdate to the next First: Last, extraction and the wait for
                // the render thread.
                super::somar("quadro_inteiro", agora.duration_since(f).as_nanos());
                super::somar("entre_quadros(extracao/render)", agora.duration_since(depois_post).as_nanos());
            }
            r.first = Some(agora);
        });
        app.add_systems(AntesUpdate, |mut r: ResMut<Relogio>| {
            let agora = Instant::now();
            if let Some(f) = r.first {
                super::somar("schedule_First..PreUpdate", agora.duration_since(f).as_nanos());
            }
            r.antes_update = Some(agora);
        });
        app.add_systems(DepoisUpdate, |mut r: ResMut<Relogio>| {
            let agora = Instant::now();
            if let Some(a) = r.antes_update {
                super::somar("schedule_Update", agora.duration_since(a).as_nanos());
            }
            r.depois_update = Some(agora);
        });
        app.add_systems(DepoisPostUpdate, |mut r: ResMut<Relogio>| {
            let agora = Instant::now();
            if let Some(a) = r.depois_update {
                super::somar("schedule_PostUpdate", agora.duration_since(a).as_nanos());
            }
            r.depois_post = Some(agora);
        });
    }
}
