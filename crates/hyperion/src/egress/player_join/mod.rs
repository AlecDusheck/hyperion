use std::{borrow::Cow, collections::BTreeSet};

use flecs_ecs::prelude::*;
use glam::{I16Vec2, IVec3};
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use tracing::{debug, info, instrument};
use valence_protocol::{
    game_mode::OptGameMode,
    ident,
    packets::play::{
        self,
        player_position_look_s2c::PlayerPositionLookFlags,
        team_s2c::{CollisionRule, Mode, NameTagVisibility, TeamColor, TeamFlags},
        GameJoinS2c,
    },
    ByteAngle, GameMode, Ident, PacketEncoder, RawBytes, VarInt, Velocity,
};
use valence_registry::{BiomeRegistry, RegistryCodec};
use valence_server::entity::EntityKind;
use valence_text::IntoText;

mod list;
pub use list::*;

use crate::{
    config::CONFIG,
    egress::metadata::show_all,
    net::{Compose, NetworkStreamRef},
    runtime::AsyncRuntime,
    simulation::{
        blocks::MinecraftWorld,
        command::{get_command_packet, Command, ROOT_COMMAND},
        skin::PlayerSkin,
        util::registry_codec_raw,
        Comms, InGameName, Play, Position, Uuid, PLAYER_SPAWN_POSITION,
    },
    system_registry::{SystemId, PLAYER_JOINS},
    util::{SendableQuery, SendableRef},
};

#[allow(clippy::too_many_arguments, reason = "todo")]
#[instrument(skip_all, fields(name = name))]
pub fn player_join_world(
    entity: &EntityView<'_>,
    tasks: &AsyncRuntime,
    chunks: &MinecraftWorld,
    compose: &Compose,
    uuid: uuid::Uuid,
    name: &str,
    packets: NetworkStreamRef,
    pose: &Position,
    world: &WorldRef<'_>,
    skin: &PlayerSkin,
    system_id: SystemId,
    root_command: Entity,
    query: &Query<(&Uuid, &InGameName, &Position, &PlayerSkin)>,
) {
    static CACHED_DATA: once_cell::sync::OnceCell<bytes::Bytes> = once_cell::sync::OnceCell::new();

    let cached_data = CACHED_DATA
        .get_or_init(|| {
            let compression_level = compose.global().shared.compression_threshold;
            let mut encoder = PacketEncoder::new();
            encoder.set_compression(compression_level);

            info!(
                "caching world data for new players with compression level {compression_level:?}"
            );
            inner(&mut encoder, chunks, tasks, world).unwrap();

            let bytes = encoder.take();
            bytes.freeze()
        })
        .clone();

    compose
        .io_buf()
        .unicast_raw(cached_data, packets, system_id, world);

    let text = play::GameMessageS2c {
        chat: format!("{name} joined the world").into_cow_text(),
        overlay: false,
    };

    compose.broadcast(&text, system_id).send(world).unwrap();

    compose
        .unicast(
            &play::PlayerPositionLookS2c {
                position: pose.position.as_dvec3(),
                yaw: pose.yaw,
                pitch: pose.pitch,
                flags: PlayerPositionLookFlags::default(),
                teleport_id: 1.into(),
            },
            packets,
            system_id,
            world,
        )
        .unwrap();

    let mut entries = Vec::new();
    let mut all_player_names = Vec::new();

    let count = query.iter_stage(world).count();

    info!("sending skins for {count} players");

    {
        let scope = tracing::trace_span!("generating_skins");
        let _enter = scope.enter();
        query.iter_stage(world).each(|(uuid, name, _, _skin)| {
            // todo: in future, do not clone

            // let PlayerSkin {
            //     textures: value,
            //     signature,
            // } = skin.clone();

            // let _property = valence_protocol::profile::Property {
            //     name: "textures".to_string(),
            //     value,
            //     signature: Some(signature),
            // };

            let entry = PlayerListEntry {
                player_uuid: uuid.0,
                username: name.to_string().into(),
                // todo: eliminate alloc
                properties: Cow::Owned(vec![]),
                chat_data: None,
                listed: true,
                ping: 20,
                game_mode: GameMode::Creative,
                display_name: Some(name.to_string().into_cow_text()),
            };

            entries.push(entry);
            all_player_names.push(name.to_string());
        });
    }

    let all_player_names = all_player_names.iter().map(String::as_str).collect();

    let actions = PlayerListActions::default()
        .with_add_player(true)
        .with_update_listed(true)
        .with_update_display_name(true);

    {
        let scope = tracing::trace_span!("unicasting_player_list");
        let _enter = scope.enter();
        compose
            .unicast(
                &PlayerListS2c {
                    actions,
                    entries: Cow::Owned(entries),
                },
                packets,
                system_id,
                world,
            )
            .unwrap();
    }

    {
        let scope = tracing::trace_span!("sending_player_spawns");
        let _enter = scope.enter();

        query
            .iter_stage(world)
            .each_iter(|it, idx, (uuid, _, pose, _)| {
                let query_entity = it.entity(idx);

                if entity.id() == query_entity.id() {
                    return;
                }

                let pkt = play::PlayerSpawnS2c {
                    entity_id: VarInt(query_entity.id().0 as i32),
                    player_uuid: uuid.0,
                    position: pose.position.as_dvec3(),
                    yaw: ByteAngle::from_degrees(pose.yaw),
                    pitch: ByteAngle::from_degrees(pose.pitch),
                };

                compose.unicast(&pkt, packets, system_id, world).unwrap();

                let show_all = show_all(query_entity.id().0 as i32);
                compose
                    .unicast(show_all.borrow_packet(), packets, system_id, world)
                    .unwrap();
            });
    }

    let PlayerSkin {
        textures,
        signature,
    } = skin.clone();

    // todo: in future, do not clone
    let property = valence_protocol::profile::Property {
        name: "textures".to_string(),
        value: textures,
        signature: Some(signature),
    };

    let property = &[property];

    let singleton_entry = &[PlayerListEntry {
        player_uuid: uuid,
        username: Cow::Borrowed(name),
        properties: Cow::Borrowed(property),
        chat_data: None,
        listed: true,
        ping: 20,
        game_mode: GameMode::Survival,
        display_name: Some(name.to_string().into_cow_text()),
    }];

    let pkt = PlayerListS2c {
        actions,
        entries: Cow::Borrowed(singleton_entry),
    };

    // todo: fix broadcasting on first tick; and this duplication can be removed!
    compose.broadcast(&pkt, system_id).send(world).unwrap();
    compose.unicast(&pkt, packets, system_id, world).unwrap();

    let player_name = vec![name];

    compose
        .broadcast(
            &play::TeamS2c {
                team_name: "no_tag",
                mode: Mode::AddEntities {
                    entities: player_name,
                },
            },
            system_id,
        )
        .exclude(packets)
        .send(world)
        .unwrap();

    let current_entity_id = VarInt(entity.id().0 as i32);

    let spawn_player = play::PlayerSpawnS2c {
        entity_id: current_entity_id,
        player_uuid: uuid,
        position: pose.position.as_dvec3(),
        yaw: ByteAngle::from_degrees(pose.yaw),
        pitch: ByteAngle::from_degrees(pose.pitch),
    };
    compose
        .broadcast(&spawn_player, system_id)
        .exclude(packets)
        .send(world)
        .unwrap();

    let show_all = show_all(entity.id().0 as i32);
    compose
        .broadcast(show_all.borrow_packet(), system_id)
        .exclude(packets)
        .send(world)
        .unwrap();

    compose
        .unicast(
            &play::TeamS2c {
                team_name: "no_tag",
                mode: Mode::AddEntities {
                    entities: all_player_names,
                },
            },
            packets,
            system_id,
            world,
        )
        .unwrap();

    let command_packet = get_command_packet(world, root_command);

    compose
        .unicast(&command_packet, packets, system_id, world)
        .unwrap();

    info!("{name} joined the world");
}

#[allow(dead_code, reason = "will re-enable")]
pub fn send_keep_alive(
    packets: NetworkStreamRef,
    compose: &Compose,
    system_id: SystemId,
    world: &World,
) -> anyhow::Result<()> {
    let pkt = play::KeepAliveS2c {
        // The ID can be set to zero because it doesn't matter
        id: 0,
    };

    compose.unicast(&pkt, packets, system_id, world)?;
    Ok(())
}

pub fn send_game_join_packet(encoder: &mut PacketEncoder) -> anyhow::Result<()> {
    // recv ack

    let registry_codec = registry_codec_raw()?;
    let codec = RegistryCodec::default();

    let dimension_names: BTreeSet<Ident<Cow<'_, str>>> = codec
        .registry(BiomeRegistry::KEY)
        .iter()
        .map(|value| value.name.as_str_ident().into())
        .collect();

    let dimension_name = ident!("overworld");
    // let dimension_name: Ident<Cow<str>> = chunk_layer.dimension_type_name().into();

    let pkt = GameJoinS2c {
        entity_id: 0,
        is_hardcore: false,
        dimension_names: Cow::Owned(dimension_names),
        registry_codec: Cow::Borrowed(&registry_codec),
        max_players: CONFIG.max_players.into(),
        view_distance: CONFIG.view_distance.into(), // max view distance
        simulation_distance: CONFIG.simulation_distance.into(),
        reduced_debug_info: false,
        enable_respawn_screen: false,
        dimension_name: dimension_name.into(),
        hashed_seed: 0,
        game_mode: GameMode::Survival,
        is_flat: false,
        last_death_location: None,
        portal_cooldown: 60.into(),
        previous_game_mode: OptGameMode(Some(GameMode::Survival)),
        dimension_type_name: "minecraft:overworld".try_into()?,
        is_debug: false,
    };

    encoder.append_packet(&pkt)?;

    Ok(())
}

fn send_sync_tags(encoder: &mut PacketEncoder) -> anyhow::Result<()> {
    let bytes = include_bytes!("data/tags.json");

    let groups = serde_json::from_slice(bytes)?;

    let pkt = play::SynchronizeTagsS2c { groups };

    encoder.append_packet(&pkt)?;

    Ok(())
}

fn inner(
    encoder: &mut PacketEncoder,
    chunks: &MinecraftWorld,
    tasks: &AsyncRuntime,
    world: &World,
) -> anyhow::Result<()> {
    send_game_join_packet(encoder)?;
    send_sync_tags(encoder)?;

    let mut buf: heapless::Vec<u8, 32> = heapless::Vec::new();
    let brand = b"discord: andrewgazelka";
    buf.push(brand.len() as u8).unwrap();
    buf.extend_from_slice(brand).unwrap();

    let bytes = RawBytes::from(buf.as_slice());

    let brand = play::CustomPayloadS2c {
        channel: ident!("minecraft:brand").into(),
        data: bytes.into(),
    };

    encoder.append_packet(&brand)?;

    let center_chunk: IVec3 = PLAYER_SPAWN_POSITION.as_ivec3() >> 4;

    // TODO: Do we need to send this else where?
    encoder.append_packet(&play::ChunkRenderDistanceCenterS2c {
        chunk_x: center_chunk.x.into(),
        chunk_z: center_chunk.z.into(),
    })?;

    let center_chunk = I16Vec2::new(center_chunk.x as i16, center_chunk.z as i16);

    // so they do not fall
    let chunk = unsafe { chunks.get_and_wait(center_chunk, tasks) };
    encoder.append_bytes(&chunk);

    // let radius = 2;

    // todo: right number?
    // let number_chunks = (radius * 2 + 1) * (radius * 2 + 1);
    //
    // (0..number_chunks).into_par_iter().for_each(|i| {
    //     let x = i % (radius * 2 + 1);
    //     let z = i / (radius * 2 + 1);
    //
    //     let x = center_chunk.x + x - radius;
    //     let z = center_chunk.z + z - radius;
    //
    //     let chunk = ChunkPos::new(x, z);
    //     if let Ok(Some(chunk)) = chunks.get(chunk, compose) {
    //         bytes_to_append.push(chunk).unwrap();
    //     }
    // });
    //
    // for elem in bytes_to_append {
    //     encoder.append_bytes(&elem);
    // }

    // send_commands(encoder)?;

    encoder.append_packet(&play::PlayerSpawnPositionS2c {
        position: PLAYER_SPAWN_POSITION.as_dvec3().into(),
        angle: 3.0,
    })?;

    encoder.append_packet(&play::TeamS2c {
        team_name: "no_tag",
        mode: Mode::CreateTeam {
            team_display_name: Cow::default(),
            friendly_flags: TeamFlags::default(),
            name_tag_visibility: NameTagVisibility::Never,
            collision_rule: CollisionRule::Always,
            team_color: TeamColor::Black,
            team_prefix: Cow::default(),
            team_suffix: Cow::default(),
            entities: vec![],
        },
    })?;

    if let Some(diameter) = CONFIG.border_diameter {
        debug!("Setting world border to diameter {}", diameter);

        encoder.append_packet(&play::WorldBorderInitializeS2c {
            x: f64::from(PLAYER_SPAWN_POSITION.x),
            z: f64::from(PLAYER_SPAWN_POSITION.z),
            old_diameter: diameter,
            new_diameter: diameter,
            duration_millis: 1.into(),
            portal_teleport_boundary: 29_999_984.into(),
            warning_blocks: 50.into(),
            warning_time: 200.into(),
        })?;

        encoder.append_packet(&play::WorldBorderSizeChangedS2c { diameter })?;

        encoder.append_packet(&play::WorldBorderCenterChangedS2c {
            x_pos: f64::from(PLAYER_SPAWN_POSITION.x),
            z_pos: f64::from(PLAYER_SPAWN_POSITION.z),
        })?;
    }

    let show_all = show_all(0);
    encoder.append_packet(show_all.borrow_packet())?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub fn spawn_entity_packet(
    id: Entity,
    kind: EntityKind,
    uuid: Uuid,
    pose: &Position,
) -> play::EntitySpawnS2c {
    info!("spawning entity");

    let entity_id = VarInt(id.0 as i32);

    play::EntitySpawnS2c {
        entity_id,
        object_uuid: *uuid,
        kind: VarInt(kind.get()),
        position: pose.position.as_dvec3(),
        pitch: ByteAngle::from_degrees(pose.pitch),
        yaw: ByteAngle::from_degrees(pose.yaw),
        head_yaw: ByteAngle::from_degrees(pose.head_yaw()),
        data: VarInt::default(),
        velocity: Velocity([0; 3]),
    }
}

#[derive(Component)]
pub struct PlayerJoinModule;

impl Module for PlayerJoinModule {
    fn module(world: &World) {
        let query = world.new_query::<(&Uuid, &InGameName, &Position, &PlayerSkin)>();

        let query = SendableQuery(query);

        let stages = (0..rayon::current_num_threads() as i32)
            // SAFETY: promoting world to static lifetime, system won't outlive world
            .map(|i| unsafe { std::mem::transmute(world.stage(i)) })
            .map(SendableRef)
            .collect::<Vec<_>>();

        let system_id = PLAYER_JOINS;

        let root_command = world.entity().set(Command::ROOT);

        ROOT_COMMAND.set(root_command.id()).unwrap();

        let hello_command = world
            .entity()
            .set(Command::literal("hello"))
            .child_of_id(root_command);

        world
            .entity()
            .set(Command::literal("world"))
            .child_of_id(hello_command);

        let root_command = root_command.id();

        system!(
            "player_joins",
            world,
            &AsyncRuntime($),
            &Comms($),
            &MinecraftWorld($),
            &Compose($),
        )
        .kind::<flecs::pipeline::PreUpdate>()
        .each(move |(tasks, comms, blocks, compose)| {
            let span = tracing::trace_span!("joins");
            let _enter = span.enter();

            let mut skins = Vec::new();

            while let Ok(Some((entity, skin))) = comms.skins_rx.try_recv() {
                skins.push((entity, skin.clone()));
            }

            // todo: par_iter but bugs...
            // for (entity, skin) in skins {
            skins.into_par_iter().for_each(|(entity, skin)| {
                // if we are not in rayon context that means we are in a single-threaded context and 0 will work
                let idx = rayon::current_thread_index().unwrap_or(0);

                let world = &stages[idx];
                let world = world.0;

                if !world.is_alive(entity) {
                    return;
                }

                let entity = world.entity_from_id(entity);

                entity.add::<Play>();

                entity.get::<(&Uuid, &InGameName, &Position, &NetworkStreamRef)>(
                    |(uuid, name, pose, &stream_id)| {
                        let query = &query;
                        let query = &query.0;

                        player_join_world(
                            &entity,
                            tasks,
                            blocks,
                            compose,
                            uuid.0,
                            name,
                            stream_id,
                            pose,
                            &world,
                            &skin,
                            system_id,
                            root_command,
                            query,
                        );
                    },
                );

                let entity = world.entity_from_id(entity);
                entity.set(skin);
            });
        });
    }
}
