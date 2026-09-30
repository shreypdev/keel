//! Crate-private macros.

/// Declares a fieldless `#[repr(u8)]` enum whose discriminants are wire tags, together with
/// `from_u8`, `as_u8`, `ALL`, `TryFrom<u8>`, `From<Enum> for u8` and a crate-private
/// `read` that decodes the tag from a [`Reader`](crate::Reader) with an accurate error offset.
macro_rules! wire_u8_enum {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $( $(#[$vmeta:meta])* $variant:ident = $value:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[repr(u8)]
        $vis enum $name {
            $( $(#[$vmeta])* $variant = $value ),+
        }

        impl $name {
            /// Every variant, in tag order.
            pub const ALL: &'static [$name] = &[ $( $name::$variant ),+ ];

            /// Returns the variant for a wire tag, or `None` if the tag is unknown.
            #[inline]
            pub const fn from_u8(tag: u8) -> Option<$name> {
                match tag {
                    $( $value => Some($name::$variant), )+
                    _ => None,
                }
            }

            /// Returns the wire tag of this variant.
            #[inline]
            pub const fn as_u8(self) -> u8 {
                self as u8
            }

            /// Reads a tag byte from `r`, reporting the real offset on failure.
            pub(crate) fn read(r: &mut $crate::Reader<'_>) -> Result<$name, $crate::WireError> {
                let at = r.position();
                let tag = r.read_u8()?;
                $name::from_u8(tag).ok_or($crate::WireError::InvalidTag {
                    tag: u32::from(tag),
                    at,
                    ty: stringify!($name),
                })
            }
        }

        impl TryFrom<u8> for $name {
            type Error = $crate::WireError;

            /// Converts a wire tag. The error carries `at: 0` because a bare byte has no
            /// position; decoders that read from a `Reader` report the real offset.
            fn try_from(tag: u8) -> Result<$name, $crate::WireError> {
                $name::from_u8(tag).ok_or($crate::WireError::InvalidTag {
                    tag: u32::from(tag),
                    at: 0,
                    ty: stringify!($name),
                })
            }
        }

        impl From<$name> for u8 {
            #[inline]
            fn from(value: $name) -> u8 {
                value.as_u8()
            }
        }
    };
}

pub(crate) use wire_u8_enum;
