//! Skill identifiers as `RuneLite` names them.

use std::fmt;

/// A skill. Names are `RuneLite`'s `Skill.getName()` values.
///
/// The set depends on the installed `RuneLite` version, so a name that is not
/// recognised is kept as [`Skill::Unknown`] instead of failing the import.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Skill {
    /// Attack.
    Attack,
    /// Defence.
    Defence,
    /// Strength.
    Strength,
    /// Hitpoints.
    Hitpoints,
    /// Ranged.
    Ranged,
    /// Prayer.
    Prayer,
    /// Magic.
    Magic,
    /// Cooking.
    Cooking,
    /// Woodcutting.
    Woodcutting,
    /// Fletching.
    Fletching,
    /// Fishing.
    Fishing,
    /// Firemaking.
    Firemaking,
    /// Crafting.
    Crafting,
    /// Smithing.
    Smithing,
    /// Mining.
    Mining,
    /// Herblore.
    Herblore,
    /// Agility.
    Agility,
    /// Thieving.
    Thieving,
    /// Slayer.
    Slayer,
    /// Farming.
    Farming,
    /// Runecraft.
    Runecraft,
    /// Hunter.
    Hunter,
    /// Construction.
    Construction,
    /// Sailing.
    Sailing,
    /// A name this version of the importer does not know.
    Unknown(String),
}

impl Skill {
    /// Parses an exact `RuneLite` skill name; anything else is [`Skill::Unknown`].
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        match name {
            "Attack" => Self::Attack,
            "Defence" => Self::Defence,
            "Strength" => Self::Strength,
            "Hitpoints" => Self::Hitpoints,
            "Ranged" => Self::Ranged,
            "Prayer" => Self::Prayer,
            "Magic" => Self::Magic,
            "Cooking" => Self::Cooking,
            "Woodcutting" => Self::Woodcutting,
            "Fletching" => Self::Fletching,
            "Fishing" => Self::Fishing,
            "Firemaking" => Self::Firemaking,
            "Crafting" => Self::Crafting,
            "Smithing" => Self::Smithing,
            "Mining" => Self::Mining,
            "Herblore" => Self::Herblore,
            "Agility" => Self::Agility,
            "Thieving" => Self::Thieving,
            "Slayer" => Self::Slayer,
            "Farming" => Self::Farming,
            "Runecraft" => Self::Runecraft,
            "Hunter" => Self::Hunter,
            "Construction" => Self::Construction,
            "Sailing" => Self::Sailing,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// The `RuneLite` name of this skill.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Attack => "Attack",
            Self::Defence => "Defence",
            Self::Strength => "Strength",
            Self::Hitpoints => "Hitpoints",
            Self::Ranged => "Ranged",
            Self::Prayer => "Prayer",
            Self::Magic => "Magic",
            Self::Cooking => "Cooking",
            Self::Woodcutting => "Woodcutting",
            Self::Fletching => "Fletching",
            Self::Fishing => "Fishing",
            Self::Firemaking => "Firemaking",
            Self::Crafting => "Crafting",
            Self::Smithing => "Smithing",
            Self::Mining => "Mining",
            Self::Herblore => "Herblore",
            Self::Agility => "Agility",
            Self::Thieving => "Thieving",
            Self::Slayer => "Slayer",
            Self::Farming => "Farming",
            Self::Runecraft => "Runecraft",
            Self::Hunter => "Hunter",
            Self::Construction => "Construction",
            Self::Sailing => "Sailing",
            Self::Unknown(name) => name,
        }
    }

    /// Whether the importer recognises this skill.
    #[must_use]
    pub const fn is_known(&self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

impl fmt::Display for Skill {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_name_round_trips() {
        for name in [
            "Attack",
            "Defence",
            "Strength",
            "Hitpoints",
            "Ranged",
            "Prayer",
            "Magic",
            "Cooking",
            "Woodcutting",
            "Fletching",
            "Fishing",
            "Firemaking",
            "Crafting",
            "Smithing",
            "Mining",
            "Herblore",
            "Agility",
            "Thieving",
            "Slayer",
            "Farming",
            "Runecraft",
            "Hunter",
            "Construction",
            "Sailing",
        ] {
            let skill = Skill::from_name(name);
            assert!(skill.is_known(), "{name}");
            assert_eq!(skill.name(), name);
        }
    }

    #[test]
    fn names_are_matched_exactly() {
        assert_eq!(
            Skill::from_name("attack"),
            Skill::Unknown("attack".to_owned())
        );
        assert_eq!(
            Skill::from_name("Overall"),
            Skill::Unknown("Overall".to_owned())
        );
    }
}
