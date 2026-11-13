package app.musicremote

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.withTimeoutOrNull
import java.net.Inet4Address
import java.util.concurrent.Executors

/** Shared and never shut down: NsdManager keeps posting cleanup tasks to it after we unregister (closing it crashed the app). */
private val nsdExecutor = Executors.newSingleThreadExecutor { r -> Thread(r, "nsd-callbacks").apply { isDaemon = true } }

/**
 * Finds the music server announced over mDNS as `_music-server._tcp` (see music-server/docs/ANDROID-CLIENT.md).
 * On Android 17 this only works once ACCESS_LOCAL_NETWORK is granted. Returns e.g. "http://192.168.1.50:8765".
 */
suspend fun discoverServer(ctx: Context, timeoutMs: Long = 8000): String? {
    val nsd = ctx.getSystemService(NsdManager::class.java) ?: return null
    val found = CompletableDeferred<String>()
    val callbacks = mutableListOf<NsdManager.ServiceInfoCallback>()

    val listener = object : NsdManager.DiscoveryListener {
        override fun onServiceFound(info: NsdServiceInfo) {
            if (!info.serviceType.contains("_music-server")) return
            val cb = object : NsdManager.ServiceInfoCallback {
                override fun onServiceUpdated(i: NsdServiceInfo) {
                    val ip = i.hostAddresses.firstOrNull { it is Inet4Address } ?: return
                    found.complete("http://${ip.hostAddress}:${i.port}")
                }
                override fun onServiceLost() {}
                override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {}
                override fun onServiceInfoCallbackUnregistered() {}
            }
            synchronized(callbacks) { callbacks += cb }
            runCatching { nsd.registerServiceInfoCallback(info, nsdExecutor, cb) }
        }
        override fun onServiceLost(info: NsdServiceInfo) {}
        override fun onDiscoveryStarted(serviceType: String) {}
        override fun onDiscoveryStopped(serviceType: String) {}
        override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) { found.cancel() }
        override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
    }

    return try {
        nsd.discoverServices("_music-server._tcp", NsdManager.PROTOCOL_DNS_SD, listener)
        withTimeoutOrNull(timeoutMs) { runCatching { found.await() }.getOrNull() }
    } finally {
        runCatching { nsd.stopServiceDiscovery(listener) }
        synchronized(callbacks) { callbacks.forEach { runCatching { nsd.unregisterServiceInfoCallback(it) } } }
    }
}
