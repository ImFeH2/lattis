package im.feh2.tun.device

import android.app.Activity
import android.net.VpnService
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class OpenArgs {
    var name: String? = null
    var addresses: List<String> = emptyList()
    var routes: List<String> = emptyList()
}

@TauriPlugin
class TunDevicePlugin(private val activity: Activity) : Plugin(activity) {
    private var pendingOpenArgs: OpenArgs? = null

    @Command
    fun open(invoke: Invoke) {
        val args = invoke.parseArgs(OpenArgs::class.java)

        if (args.addresses.isEmpty()) {
            invoke.reject("At least one TUN address must be configured")
            return
        }

        val permissionIntent = VpnService.prepare(activity)
        if (permissionIntent != null) {
            if (pendingOpenArgs != null) {
                invoke.reject("Another VPN permission request is already pending")
                return
            }

            pendingOpenArgs = args
            startActivityForResult(invoke, permissionIntent, "handleVpnPermission")
            return
        }

        openVpn(invoke, args)
    }

    @ActivityCallback
    fun handleVpnPermission(invoke: Invoke, result: ActivityResult) {
        val args = pendingOpenArgs
        pendingOpenArgs = null

        if (result.resultCode != Activity.RESULT_OK) {
            invoke.reject("VPN permission was not granted")
            return
        }

        if (args == null) {
            invoke.reject("VPN configuration is no longer available")
            return
        }

        openVpn(invoke, args)
    }

    private fun openVpn(invoke: Invoke, args: OpenArgs) {
        TunDeviceService.open(activity.applicationContext, args, object : TunDeviceService.OpenCallback {
            override fun onOpened(fd: Int) {
                val result = JSObject()
                result.put("fd", fd)
                invoke.resolve(result)
            }

            override fun onError(message: String) {
                invoke.reject(message)
            }
        })
    }
}
