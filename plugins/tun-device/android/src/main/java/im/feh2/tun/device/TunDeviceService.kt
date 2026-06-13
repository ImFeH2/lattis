package im.feh2.tun.device

import android.content.Context
import android.content.Intent
import android.net.VpnService

class TunDeviceService : VpnService() {
    interface OpenCallback {
        fun onOpened(fd: Int)
        fun onError(message: String)
    }

    private data class PendingOpen(
        val args: OpenArgs,
        val callback: OpenCallback,
    )

    private data class Cidr(
        val address: String,
        val prefixLength: Int,
    )

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action != ACTION_OPEN) {
            return START_STICKY
        }

        val pending = pendingOpen
        pendingOpen = null

        if (pending == null) {
            stopSelf(startId)
            return START_NOT_STICKY
        }

        try {
            val descriptor = establish(pending.args)
            val fd = descriptor.detachFd()
            pending.callback.onOpened(fd)
        } catch (error: Exception) {
            pending.callback.onError(error.message ?: "Failed to create TUN device")
            stopSelf(startId)
        }

        return START_STICKY
    }

    private fun establish(args: OpenArgs): android.os.ParcelFileDescriptor {
        val sessionName = args.name?.takeIf { it.isNotBlank() } ?: "Lattis"
        val builder = Builder()
            .setSession(sessionName)
            .setBlocking(false)

        args.addresses
            .map(::parseCidr)
            .forEach { builder.addAddress(it.address, it.prefixLength) }

        val routes = args.routes.ifEmpty { args.addresses }
        routes
            .map(::parseCidr)
            .forEach { builder.addRoute(it.address, it.prefixLength) }

        return builder.establish()
            ?: throw IllegalStateException("Android did not create a TUN device")
    }

    private fun parseCidr(value: String): Cidr {
        val parts = value.split("/", limit = 2)
        if (parts.size != 2 || parts[0].isBlank()) {
            throw IllegalArgumentException("Invalid CIDR address: $value")
        }

        val prefixLength = parts[1].toIntOrNull()
            ?: throw IllegalArgumentException("Invalid CIDR prefix length: $value")

        return Cidr(parts[0], prefixLength)
    }

    companion object {
        private const val ACTION_OPEN = "im.feh2.tun.device.OPEN"

        @Volatile
        private var pendingOpen: PendingOpen? = null

        fun open(context: Context, args: OpenArgs, callback: OpenCallback) {
            pendingOpen = PendingOpen(args, callback)

            val intent = Intent(context, TunDeviceService::class.java)
                .setAction(ACTION_OPEN)

            context.startService(intent)
        }
    }
}
