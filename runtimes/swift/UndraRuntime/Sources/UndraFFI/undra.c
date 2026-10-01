/*
 * undra.c - the one source file of the UndraFFI target.
 *
 * UndraFFI is header-only: `include/undra.h` declares the C ABI's types (the `UndraApi` table of
 * one core, `UndraBuf` and the callback types) and no function, so the runtime references no
 * symbol of any core (ADR-044). SwiftPM needs a source file to build a C target; this one only
 * checks that the header compiles on its own.
 */
#include "include/undra.h"
