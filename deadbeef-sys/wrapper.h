#define DDB_API_LEVEL 17
#include "deadbeef.h"

typedef struct DB_hotkeys_plugin_s {
    DB_misc_t misc;
    const char *(*get_name_for_keycode) (int keycode);
    void (*reset) (void);
    // since plugin version 1.1
    DB_plugin_action_t *(*get_action_for_keycombo) (int key, int mods, int isglobal, ddb_action_context_t *ctx);
} DB_hotkeys_plugin_t;

