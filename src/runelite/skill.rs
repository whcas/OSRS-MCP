//! Skill identifiers as `RuneLite` names them.

use std::fmt;

/// Declares [`Skill`] from one list of names. Each variant's name is exactly
/// the `RuneLite` skill name (`Skill.getName()`), so the enum, the parser and
/// the printer cannot drift apart.
macro_rules! skills {
    ($($name:ident),+ $(,)?) => {
        /// A skill. Names are `RuneLite`'s `Skill.getName()` values.
        ///
        /// The set depends on the installed `RuneLite` version, so a name that
        /// is not recognised is kept as [`Skill::Unknown`] instead of failing
        /// the import.
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum Skill {
            $(
                #[doc = concat!("The `", stringify!($name), "` skill.")]
                $name,
            )+
            /// A name this version of the importer does not know.
            Unknown(String),
        }

        impl Skill {
            /// The names of every recognised skill, in declaration order.
            pub const KNOWN_NAMES: &'static [&'static str] = &[$(stringify!($name)),+];

            /// Parses an exact `RuneLite` skill name; anything else is
            /// [`Skill::Unknown`].
            #[must_use]
            pub fn from_name(name: &str) -> Self {
                match name {
                    $(stringify!($name) => Self::$name,)+
                    other => Self::Unknown(other.to_owned()),
                }
            }

            /// The `RuneLite` name of this skill.
            #[must_use]
            pub fn name(&self) -> &str {
                match self {
                    $(Self::$name => stringify!($name),)+
                    Self::Unknown(name) => name,
                }
            }
        }
    };
}

skills![
    Attack,
    Defence,
    Strength,
    Hitpoints,
    Ranged,
    Prayer,
    Magic,
    Cooking,
    Woodcutting,
    Fletching,
    Fishing,
    Firemaking,
    Crafting,
    Smithing,
    Mining,
    Herblore,
    Agility,
    Thieving,
    Slayer,
    Farming,
    Runecraft,
    Hunter,
    Construction,
    Sailing,
];

impl Skill {
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
        assert_eq!(Skill::KNOWN_NAMES.len(), 24);
        for name in Skill::KNOWN_NAMES {
            let skill = Skill::from_name(name);
            assert!(skill.is_known(), "{name}");
            assert_eq!(skill.name(), *name);
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
