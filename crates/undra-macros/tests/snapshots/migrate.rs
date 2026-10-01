fn todo_v1(old: &DynValue) -> Result<Todo, MigrateError> {
    unimplemented!()
}
#[doc(hidden)]
#[allow(non_snake_case, non_camel_case_types, dead_code, unused, clippy::all)]
const _: () = {
    fn __undra_migrate_todo_v1(
        __old: &::undra::runtime::persist::DynValue,
    ) -> ::core::result::Result<
        ::std::vec::Vec<u8>,
        ::undra::runtime::persist::MigrateError,
    > {
        todo_v1(__old).map(|__v| ::undra::wire::Encode::encode_to_vec(&__v))
    }
    ::undra::runtime::inventory::submit! {
        ::undra::runtime::persist::Migration { name :
        ::core::concat!(::core::module_path!(), "::", "todo_v1"), target :
        ::undra::runtime::persist::MigrationTarget::Type("Todo"), from :
        ::core::option::Option::Some(255u64), returns :
        ::core::option::Option::Some(::undra::meta::TypeRefMeta::Named("Todo")), hook :
        ::undra::runtime::persist::MigrationHook::Value(__undra_migrate_todo_v1), support
        : ::undra::runtime::persist::HOOK_SUPPORT, }
    }
};
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused, clippy::all)]
const _: () = {
    trait __UndraFallback {
        const UNDRA_TYPE_ID: u32 = 0;
    }
    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
    const _: () = {
        let __undra_id = <Todo>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0066]: the migration hook `todo_v1` returns `Todo`, which is not a record or enum declared with `#[undra::api]`\n  = note: a migration hook converts persisted data of one record or enum, one signal of one store, or one mutation's queued input to this build's types; the runtime finds it by that target and calls it with the old value in a fixed shape\n  = help: a `ty` hook targets and returns a record or enum of the schema: declare `Todo` with `#[undra::api]` if it is one, or return the `#[undra::api]` type `Todo` names\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0066"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Todo") {
            ::core::panic!(
                "error[undra::E0066]: the migration hook `todo_v1` targets `Todo` but returns `Todo`\n  = note: a migration hook converts persisted data of one record or enum, one signal of one store, or one mutation's queued input to this build's types; the runtime finds it by that target and calls it with the old value in a fixed shape\n  = help: return the current `Todo` (the hook builds today's value from the old one), or target the type it returns: `ty = \"Todo\"`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0066"
            );
        }
    };
};
fn age(old: Option<&DynValue>) -> Result<f32, MigrateError> {
    Ok(18.0)
}
#[doc(hidden)]
#[allow(non_snake_case, non_camel_case_types, dead_code, unused, clippy::all)]
const _: () = {
    fn __undra_migrate_age(
        __old: ::core::option::Option<&::undra::runtime::persist::DynValue>,
    ) -> ::core::result::Result<
        ::std::vec::Vec<u8>,
        ::undra::runtime::persist::MigrateError,
    > {
        age(__old).map(|__v| ::undra::wire::Encode::encode_to_vec(&__v))
    }
    ::undra::runtime::inventory::submit! {
        ::undra::runtime::persist::Migration { name :
        ::core::concat!(::core::module_path!(), "::", "age"), target :
        ::undra::runtime::persist::MigrationTarget::Signal { store : "Profile", signal :
        "age" }, from : ::core::option::Option::None, returns :
        ::core::option::Option::Some(::undra::meta::TypeRefMeta::F32), hook :
        ::undra::runtime::persist::MigrationHook::Signal(__undra_migrate_age), support :
        ::undra::runtime::persist::HOOK_SUPPORT, }
    }
};
fn add_todo(old: &DynRecord) -> Result<DynRecord, MigrateError> {
    Ok(old.clone())
}
#[doc(hidden)]
#[allow(non_snake_case, non_camel_case_types, dead_code, unused, clippy::all)]
const _: () = {
    ::undra::runtime::inventory::submit! {
        ::undra::runtime::persist::Migration { name :
        ::core::concat!(::core::module_path!(), "::", "add_todo"), target :
        ::undra::runtime::persist::MigrationTarget::Mutation("add_todo"), from :
        ::core::option::Option::None, returns : ::core::option::Option::None, hook :
        ::undra::runtime::persist::MigrationHook::Mutation(add_todo), support :
        ::undra::runtime::persist::HOOK_SUPPORT, }
    }
};
