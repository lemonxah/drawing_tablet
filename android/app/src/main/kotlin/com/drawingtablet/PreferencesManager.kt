package com.drawingtablet

import android.content.Context
import android.content.SharedPreferences

class PreferencesManager(context: Context) {
    private val prefs: SharedPreferences = context.getSharedPreferences("drawing_tablet_prefs", Context.MODE_PRIVATE)

    var rememberedServerHost: String?
        get() = prefs.getString("remembered_host", null)
        set(value) = prefs.edit().putString("remembered_host", value).apply()

    var rememberedServerPort: Int
        get() = prefs.getInt("remembered_port", 0)
        set(value) = prefs.edit().putInt("remembered_port", value).apply()

    var shouldAutoConnect: Boolean
        get() = prefs.getBoolean("auto_connect", false)
        set(value) = prefs.edit().putBoolean("auto_connect", value).apply()

    fun clearRememberedServer() {
        prefs.edit()
            .remove("remembered_host")
            .remove("remembered_port")
            .remove("auto_connect")
            .apply()
    }
}
