use flecs_ecs::core::{World, WorldGet};
use hyperion_inventory::PlayerInventory;
use hyperion_item::builder::{AttackDamage, BookBuilder, Color, ItemBuilder};
use valence_protocol::ItemKind;

use crate::{Handles, Rank, Team};

impl Team {
    pub const fn build_item(self) -> ItemBuilder {
        let kind = match self {
            Self::Blue => ItemKind::BlueTerracotta,
            Self::Green => ItemKind::GreenTerracotta,
            Self::Red => ItemKind::RedTerracotta,
            Self::Yellow => ItemKind::YellowTerracotta,
        };

        ItemBuilder::new(kind)
    }
}

pub const MAIN_SLOT: u16 = 0;
pub const PICKAXE_SLOT: u16 = 1;
pub const BLOCK_SLOT: u16 = 2;
pub const UPGRADE_START_SLOT: u16 = 3;
pub const GUI_SLOT: u16 = 7;
pub const HELP_SLOT: u16 = 8;

impl Rank {
    pub fn apply_inventory(
        self,
        team: Team,
        inventory: &mut PlayerInventory,
        world: &World,
        build_count: i8,
        extra_levels: u8,
    ) {
        inventory.clear();
        let upgrade_not_available = ItemBuilder::new(ItemKind::GrayDye);

        let book = BookBuilder::new("§b@andrewgazelka", "§6§l10k Guide")
            .add_page(
                "§6Welcome to Hyperion!\n\n§7This is a §c10,000§7 player PvP battle to break the \
                 Guinness World Record!\n\n§7Current record: §b8,825 players",
            )
            .add_page(
                "§6§lTeams\n\n§cRed Team\n§9Blue Team\n§aGreen Team\n§6Yellow Team\n\n§7Teams are \
                 identified by boot color!",
            )
            .add_page(
                "§6§lProgression\n\n§7Gain XP by:\n§7- Mining ores\n§7- Killing players\n\n§7When \
                 killed:\n§7- Keep §61/3§7 of XP\n§7- Killer gets §61/2§7 of your XP",
            )
            .add_page(
                "§6§lClasses\n\n§7Everyone starts with the §dStick§7 class\n\n§7Unlock new \
                 classes by gaining XP and defeating players!\n\n§7Upgrade your gear to become \
                 stronger!",
            )
            .add_page(
                "§6§lControls\n\n§7[1-4] §7Combat Items\n§7[5-6] §7Building Blocks\n§7[7] \
                 §7Upgrades Menu\n§7[8] §7Help Book\n\n§6Good luck!",
            )
            .build();

        inventory.set_hotbar(HELP_SLOT, book);

        let color = match team {
            Team::Red => Color(255, 0, 0),
            Team::Blue => Color(0, 0, 255),
            Team::Green => Color(0, 255, 0),
            Team::Yellow => Color(255, 255, 0),
        };

        let boots = ItemBuilder::new(ItemKind::LeatherBoots)
            .color(color)
            .build();

        inventory.set_boots(boots);

        let upgrades = ["Speed", "Health", "Armor", "Damage"]
            .into_iter()
            .map(|title| match extra_levels {
                0 => format!("§6§l{title}"),
                _ => format!("§6§l{title}§e §c({extra_levels})"),
            });

        let upgrade_available = [
            ItemKind::BlueDye,   // Speed
            ItemKind::RedDye,    // Health
            ItemKind::YellowDye, // Armor
            ItemKind::GreenDye,  // Damage
        ];

        world.get::<&Handles>(|handles| {
            for ((i, upgrade), available_item) in upgrades.enumerate().zip(upgrade_available) {
                let slot = u16::try_from(i).unwrap() + UPGRADE_START_SLOT;
                let mut item = upgrade_not_available
                    .clone()
                    .name(upgrade)
                    .handler(handles.speed);

                if extra_levels > 0 {
                    item = item.kind(available_item);
                }

                let item = item.build();
                inventory.set_hotbar(slot, item);
            }
        });

        let default_pickaxe = ItemBuilder::new(ItemKind::WoodenPickaxe).build();
        inventory.set_hotbar(PICKAXE_SLOT, default_pickaxe);

        let default_build_item = team.build_item().count(build_count).build();
        inventory.set_hotbar(BLOCK_SLOT, default_build_item);

        match self {
            Self::Stick => {
                let stick = ItemBuilder::new(ItemKind::Stick)
                    .glowing()
                    .name("§dG§5R§dO§5N§dK§5-§dTHUNK")
                    .add_attribute(AttackDamage(3.0))
                    .build();

                inventory.set_hotbar(MAIN_SLOT, stick);
            }

            Self::Archer => {
                let bow = ItemBuilder::new(ItemKind::Bow).build();

                inventory.set_hotbar(MAIN_SLOT, bow);

                let arrow = ItemBuilder::new(ItemKind::Arrow).count(64).build();

                inventory.set(9, arrow).unwrap();
            }
            Self::Sword => {
                let sword = ItemBuilder::new(ItemKind::StoneSword)
                    .name("§3VERTEX§f-§bSLICER")
                    .add_attribute(AttackDamage(3.0))
                    .build();

                inventory.set_hotbar(MAIN_SLOT, sword);
            }

            Self::Miner => {
                let torch = ItemBuilder::new(ItemKind::Torch)
                    .name("§3§lTORCH")
                    .glowing()
                    .add_attribute(AttackDamage(3.0))
                    .build();

                inventory.set_hotbar(MAIN_SLOT, torch);

                let pickaxe = ItemBuilder::new(ItemKind::StonePickaxe)
                    .add_attribute(AttackDamage(2.0))
                    .build();

                inventory.set_hotbar(PICKAXE_SLOT, pickaxe);
            }

            Self::Mage => {
                let wand = ItemBuilder::new(ItemKind::WoodenShovel)
                    .glowing()
                    .add_attribute(AttackDamage(2.0))
                    .build();

                inventory.set_hotbar(MAIN_SLOT, wand);
            }
            Self::Knight => {
                let knight_sword = ItemBuilder::new(ItemKind::IronSword)
                    .name("§7§k-§r§7BESKAR-§8BLADE")
                    .glowing()
                    .add_attribute(AttackDamage(4.0))
                    .build();

                inventory.set_hotbar(MAIN_SLOT, knight_sword);
            }
            Self::Builder => {
                let builder_tool = ItemBuilder::new(ItemKind::GoldenPickaxe)
                    .add_attribute(AttackDamage(5.0))
                    .build();

                inventory.set_hotbar(MAIN_SLOT, builder_tool);
            }
            Self::Excavator => {
                let pickaxe = ItemBuilder::new(ItemKind::IronPickaxe).build();

                inventory.set_hotbar(PICKAXE_SLOT, pickaxe);

                let minecart = ItemBuilder::new(ItemKind::Minecart)
                    .name("§3§lMINECART")
                    .glowing()
                    .add_attribute(AttackDamage(3.0))
                    .build();

                inventory.set_hotbar(MAIN_SLOT, minecart);
            }
        }

        let upgrade_item = ItemBuilder::new(ItemKind::FireworkStar)
            .name("Upgrades")
            .build();

        inventory.set_hotbar(GUI_SLOT, upgrade_item);
    }
}
