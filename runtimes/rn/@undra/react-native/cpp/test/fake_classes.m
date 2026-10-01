// The classes the linked shim finds the fake cores of fake_cores.c by (what a core's pod compiles,
// `<Bundle>Table.m`, written by `undra build --platform rn`), and one without `+api`.
#import <Foundation/Foundation.h>

#define UNDRA_FAKE_CORE(ns)                     \
  const void *ns##_undra_api(void);             \
  @interface UndraCoreTable_##ns : NSObject     \
  + (const void *)api;                          \
  @end                                          \
  @implementation UndraCoreTable_##ns           \
  + (const void *)api {                         \
    return ns##_undra_api();                    \
  }                                             \
  @end

UNDRA_FAKE_CORE(bad_abi)
UNDRA_FAKE_CORE(short_table)
UNDRA_FAKE_CORE(wrong_ns)
UNDRA_FAKE_CORE(null_entry)

@interface UndraCoreTable_not_a_core : NSObject
@end
@implementation UndraCoreTable_not_a_core
@end
