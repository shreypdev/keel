"""Unit tests of how an action decides which files it copies (ADR-061)."""

load("@bazel_skylib//lib:unittest.bzl", "asserts", "unittest")
load("//undra/private:actions.bzl", "below")

def _below_test(ctx):
    env = unittest.begin(ctx)

    # The root of the main repository: every file of it, nothing of another repository.
    asserts.equals(env, "core/src/lib.rs", below("core/src/lib.rs", ""))
    asserts.equals(env, "undra.toml", below("undra.toml", ""))
    asserts.equals(env, None, below("../undra_rules++undra+undra/Cargo.toml", ""), "another repository is not below the main one")

    # A package: its files, not a sibling whose name it prefixes.
    asserts.equals(env, "core/Cargo.toml", below("app/core/Cargo.toml", "app"))
    asserts.equals(env, None, below("apps/core/Cargo.toml", "app"), "`apps` is not below `app`")
    asserts.equals(env, None, below("kotlin/HelloTest.kt", "app"))

    # Another repository, by its short path.
    asserts.equals(env, "crates/undra/Cargo.toml", below("../undra_rules++undra+undra/crates/undra/Cargo.toml", "../undra_rules++undra+undra"))
    return unittest.end(env)

below_test = unittest.make(_below_test)

def actions_test_suite(name):
    unittest.suite(name, below_test)
