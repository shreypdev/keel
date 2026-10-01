//! The call payload (SPEC 3.3).

use crate::{Handle, Reader, WireError, Writer};

/// What a [`Call`] invokes.
///
/// | target | variant | layout after the `target` byte |
/// |---|---|---|
/// | 0 | `Function` | `handle u64 (reserved, 0), method_id u32, call_id u32, args` |
/// | 1 | `Method` | `handle u64, method_id u32, call_id u32, args` |
/// | 2 | `Constructor` | `type_id u32, method_id u32, call_id u32, args` |
/// | 3 | `LazyPage` | `handle u64, offset u32, limit u32, call_id u32` (no args) |
///
/// Note that constructors have no handle field, unlike free functions; this follows the
/// explicit layout in SPEC 3.3 rather than the summary table above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallTarget {
    /// A free function (`#[undra::api] fn`).
    Function {
        /// `fnv1a32("fn.<name>")`.
        method_id: u32,
    },
    /// A method on an object.
    Method {
        /// The receiver.
        handle: Handle,
        /// `fnv1a32("<TypeName>.<method>")`.
        method_id: u32,
    },
    /// A constructor of an object type.
    Constructor {
        /// `fnv1a32("<TypeName>")`.
        type_id: u32,
        /// Selects the constructor.
        method_id: u32,
    },
    /// A page of a lazy list.
    LazyPage {
        /// The lazy-list object.
        handle: Handle,
        /// Index of the first item wanted.
        offset: u32,
        /// Maximum number of items wanted.
        limit: u32,
    },
}

/// A call from the host into the core (kind `Call`), borrowing its arguments.
///
/// `args` holds the encoded parameters in declaration order and is the rest of the payload.
/// `LazyPage` calls carry no arguments: `args` is ignored when encoding and empty when
/// decoding.
///
/// On decode the handle field of a `Function` call is reserved and ignored; encoders write `0`.
///
/// Use [`CallOwned`] when the call must outlive the buffer it was decoded from.
///
/// # Example
///
/// ```
/// use undra_wire::payload::{Call, CallTarget};
/// use undra_wire::{Handle, Reader, Writer};
///
/// let call = Call {
///     target: CallTarget::Method { handle: Handle::new(1, 1), method_id: 0x8c4540e0 },
///     call_id: 9,
///     args: &[2, 0, 0, 0, 3, 0, 0, 0],
/// };
/// let mut w = Writer::new();
/// call.encode(&mut w);
/// assert_eq!(Call::decode(&mut Reader::new(w.as_slice())), Ok(call));
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call<'a> {
    /// What is being called.
    pub target: CallTarget,
    /// Chosen by the host, unique among in-flight calls; `0` is reserved.
    pub call_id: u32,
    /// The encoded parameters.
    pub args: &'a [u8],
}

impl<'a> Call<'a> {
    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        w.reserve(21 + self.args.len());
        match self.target {
            CallTarget::Function { method_id } => {
                w.write_u8(0);
                w.write_u64(0);
                w.write_u32(method_id);
                w.write_u32(self.call_id);
                w.write_raw(self.args);
            }
            CallTarget::Method { handle, method_id } => {
                w.write_u8(1);
                w.write_u64(handle.0);
                w.write_u32(method_id);
                w.write_u32(self.call_id);
                w.write_raw(self.args);
            }
            CallTarget::Constructor { type_id, method_id } => {
                w.write_u8(2);
                w.write_u32(type_id);
                w.write_u32(method_id);
                w.write_u32(self.call_id);
                w.write_raw(self.args);
            }
            CallTarget::LazyPage {
                handle,
                offset,
                limit,
            } => {
                w.write_u8(3);
                w.write_u64(handle.0);
                w.write_u32(offset);
                w.write_u32(limit);
                w.write_u32(self.call_id);
            }
        }
    }

    /// Reads a payload. For targets 0 to 2 `args` is everything after `call_id`.
    ///
    /// An unknown target byte is [`WireError::InvalidTag`] with `ty: "CallTarget"`.
    pub fn decode(r: &mut Reader<'a>) -> Result<Self, WireError> {
        let at = r.position();
        match r.read_u8()? {
            0 => {
                let _reserved_handle = r.read_u64()?;
                let method_id = r.read_u32()?;
                let call_id = r.read_u32()?;
                Ok(Call {
                    target: CallTarget::Function { method_id },
                    call_id,
                    args: r.read_rest(),
                })
            }
            1 => {
                let handle = Handle(r.read_u64()?);
                let method_id = r.read_u32()?;
                let call_id = r.read_u32()?;
                Ok(Call {
                    target: CallTarget::Method { handle, method_id },
                    call_id,
                    args: r.read_rest(),
                })
            }
            2 => {
                let type_id = r.read_u32()?;
                let method_id = r.read_u32()?;
                let call_id = r.read_u32()?;
                Ok(Call {
                    target: CallTarget::Constructor { type_id, method_id },
                    call_id,
                    args: r.read_rest(),
                })
            }
            3 => {
                let handle = Handle(r.read_u64()?);
                let offset = r.read_u32()?;
                let limit = r.read_u32()?;
                let call_id = r.read_u32()?;
                Ok(Call {
                    target: CallTarget::LazyPage {
                        handle,
                        offset,
                        limit,
                    },
                    call_id,
                    args: &[],
                })
            }
            tag => Err(WireError::InvalidTag {
                tag: u32::from(tag),
                at,
                ty: "CallTarget",
            }),
        }
    }
}

/// An owned [`Call`]: the same fields with `args` in a `Vec<u8>`.
///
/// Convert with [`CallOwned::from`] (copying `args`) and [`CallOwned::as_call`] (borrowing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallOwned {
    /// What is being called.
    pub target: CallTarget,
    /// Chosen by the host, unique among in-flight calls; `0` is reserved.
    pub call_id: u32,
    /// The encoded parameters.
    pub args: Vec<u8>,
}

impl CallOwned {
    /// Borrows this call as a [`Call`].
    pub fn as_call(&self) -> Call<'_> {
        Call {
            target: self.target,
            call_id: self.call_id,
            args: &self.args,
        }
    }

    /// Appends the payload to `w`.
    pub fn encode(&self, w: &mut Writer) {
        self.as_call().encode(w);
    }

    /// Reads a payload, copying `args` out of the input.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Call::decode(r).map(|call| CallOwned::from(&call))
    }
}

impl From<&Call<'_>> for CallOwned {
    fn from(call: &Call<'_>) -> Self {
        CallOwned {
            target: call.target,
            call_id: call.call_id,
            args: call.args.to_vec(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(call: &Call<'_>) -> Vec<u8> {
        let mut w = Writer::new();
        call.encode(&mut w);
        w.into_vec()
    }

    #[test]
    fn method_layout_matches_the_vector() {
        let call = Call {
            target: CallTarget::Method {
                handle: Handle(4_294_967_297),
                method_id: 2_353_348_832,
            },
            call_id: 9,
            args: &[2, 0, 0, 0, 3, 0, 0, 0],
        };
        let b = encode(&call);
        assert_eq!(
            b,
            [
                1, 1, 0, 0, 0, 1, 0, 0, 0, 0xe0, 0x40, 0x45, 0x8c, 9, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0,
                0
            ]
        );
        assert_eq!(Call::decode(&mut Reader::new(&b)), Ok(call));
    }

    #[test]
    fn function_layout_has_a_zero_handle() {
        let call = Call {
            target: CallTarget::Function { method_id: 7 },
            call_id: 1,
            args: &[],
        };
        let b = encode(&call);
        assert_eq!(b, [0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(Call::decode(&mut Reader::new(&b)), Ok(call));
    }

    #[test]
    fn function_ignores_a_nonzero_reserved_handle() {
        let mut b = encode(&Call {
            target: CallTarget::Function { method_id: 7 },
            call_id: 1,
            args: &[],
        });
        b[1] = 0xff;
        assert_eq!(
            Call::decode(&mut Reader::new(&b)).unwrap().target,
            CallTarget::Function { method_id: 7 }
        );
    }

    #[test]
    fn constructor_layout_has_no_handle() {
        let call = Call {
            target: CallTarget::Constructor {
                type_id: 1,
                method_id: 2,
            },
            call_id: 3,
            args: &[9],
        };
        let b = encode(&call);
        assert_eq!(b, [2, 1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 9]);
        assert_eq!(Call::decode(&mut Reader::new(&b)), Ok(call));
    }

    #[test]
    fn lazy_page_layout_has_no_args() {
        let call = Call {
            target: CallTarget::LazyPage {
                handle: Handle::new(1, 1),
                offset: 20,
                limit: 10,
            },
            call_id: 5,
            args: &[],
        };
        let b = encode(&call);
        assert_eq!(
            b,
            [
                3, 1, 0, 0, 0, 1, 0, 0, 0, 20, 0, 0, 0, 10, 0, 0, 0, 5, 0, 0, 0
            ]
        );
        let mut r = Reader::new(&b);
        assert_eq!(Call::decode(&mut r), Ok(call));
        assert!(r.finish().is_ok());
    }

    #[test]
    fn lazy_page_args_are_ignored_on_encode() {
        let call = Call {
            target: CallTarget::LazyPage {
                handle: Handle(1),
                offset: 0,
                limit: 1,
            },
            call_id: 1,
            args: &[1, 2, 3],
        };
        assert_eq!(encode(&call).len(), 21);
    }

    #[test]
    fn unknown_target_is_rejected() {
        assert_eq!(
            Call::decode(&mut Reader::new(&[4, 0, 0, 0])),
            Err(WireError::InvalidTag {
                tag: 4,
                at: 0,
                ty: "CallTarget"
            })
        );
    }

    #[test]
    fn truncation_is_an_error_at_every_length() {
        let call = Call {
            target: CallTarget::Method {
                handle: Handle(1),
                method_id: 2,
            },
            call_id: 3,
            args: &[],
        };
        let b = encode(&call);
        for cut in 0..b.len() {
            assert!(Call::decode(&mut Reader::new(&b[..cut])).is_err(), "{cut}");
        }
    }

    #[test]
    fn args_borrow_from_the_input() {
        let call = Call {
            target: CallTarget::Function { method_id: 1 },
            call_id: 2,
            args: &[5, 6, 7],
        };
        let b = encode(&call);
        let decoded = Call::decode(&mut Reader::new(&b)).unwrap();
        assert_eq!(decoded.args.as_ptr(), b[b.len() - 3..].as_ptr());
    }

    #[test]
    fn owned_round_trip() {
        let b = encode(&Call {
            target: CallTarget::Function { method_id: 1 },
            call_id: 2,
            args: &[5, 6, 7],
        });
        let owned = CallOwned::decode(&mut Reader::new(&b)).unwrap();
        assert_eq!(owned.args, [5, 6, 7]);
        let mut w = Writer::new();
        owned.encode(&mut w);
        assert_eq!(w.as_slice(), &b[..]);
        assert_eq!(CallOwned::from(&owned.as_call()), owned);
    }
}
