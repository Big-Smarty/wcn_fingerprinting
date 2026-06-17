package de.hfu.wcnfingerprinting

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.location.LocationManager
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import androidx.activity.result.ActivityResult
import androidx.core.content.ContextCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import org.json.JSONArray
import org.json.JSONObject

@TauriPlugin
class WcnPlugin(private val activity: Activity) : Plugin(activity) {
  private val wifiManager: WifiManager by lazy {
    activity.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
  }

  @Command
  fun scanWifiSamples(invoke: Invoke) {
    if (!hasRuntimePermissions()) {
      handle!!.requestPermissions(invoke, requiredRuntimePermissions(), "scanWifiSamplesAfterPermission")
      return
    }

    beginScan(invoke)
  }

  @PermissionCallback
  fun scanWifiSamplesAfterPermission(invoke: Invoke) {
    if (!hasRuntimePermissions()) {
      invoke.reject("Location and nearby Wi-Fi permissions are required for Wi-Fi fingerprinting.")
      return
    }

    beginScan(invoke)
  }

  @Command
  fun saveBackup(invoke: Invoke) {
    val args = invoke.getArgs()
    val suggestedName = args.getString("suggestedName", "wcn-fingerprints.surql")
    val sourcePath = args.getString("sourcePath", null)

    if (sourcePath.isNullOrBlank()) {
      invoke.reject("Missing backup source path.")
      return
    }

    val sourceFile = File(sourcePath)
    if (!sourceFile.exists() || !sourceFile.isFile) {
      invoke.reject("Backup source file does not exist.")
      return
    }

    val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
      addCategory(Intent.CATEGORY_OPENABLE)
      type = "application/octet-stream"
      putExtra(Intent.EXTRA_TITLE, suggestedName)
    }

    startActivityForResult(invoke, intent, "saveBackupResult")
  }

  @ActivityCallback
  fun saveBackupResult(invoke: Invoke, result: ActivityResult) {
    if (result.resultCode != Activity.RESULT_OK) {
      invoke.reject("Backup export canceled.")
      return
    }

    val destination = result.data?.data
    if (destination == null) {
      invoke.reject("No backup destination was selected.")
      return
    }

    val sourcePath = invoke.getArgs().getString("sourcePath", null)
    if (sourcePath.isNullOrBlank()) {
      invoke.reject("Missing backup source path.")
      return
    }

    try {
      val bytes = copyToUri(File(sourcePath), destination)
      val response = JSObject()
      response.put("uri", destination.toString())
      response.put("bytes", bytes)
      invoke.resolve(response)
    } catch (error: Exception) {
      invoke.reject("Could not write backup: ${error.message}", error)
    }
  }

  private fun beginScan(invoke: Invoke) {
    val args = invoke.getArgs()
    val sampleCount = args.getInteger("sampleCount", 4).coerceIn(1, 16)

    val readinessError = readinessError()
    if (readinessError != null) {
      invoke.reject(readinessError)
      return
    }

    Thread {
      try {
        val response = collectFreshScans(sampleCount)
        invoke.resolve(response)
      } catch (error: Exception) {
        invoke.reject(error.message ?: "Wi-Fi scan failed.", error)
      }
    }.start()
  }

  private fun requiredRuntimePermissions(): Array<String> {
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.NEARBY_WIFI_DEVICES)
    } else {
      arrayOf(Manifest.permission.ACCESS_FINE_LOCATION)
    }
  }

  private fun hasRuntimePermissions(): Boolean {
    return requiredRuntimePermissions().all { permission ->
      ContextCompat.checkSelfPermission(activity, permission) == android.content.pm.PackageManager.PERMISSION_GRANTED
    }
  }

  @Suppress("DEPRECATION")
  private fun readinessError(): String? {
    if (!wifiManager.isWifiEnabled) {
      return "Wi-Fi is disabled. Enable Wi-Fi on the phone before fingerprinting."
    }

    if (!isLocationEnabled()) {
      return "Android Location services are disabled. Enable Location before scanning Wi-Fi BSSIDs."
    }

    return null
  }

  private fun isLocationEnabled(): Boolean {
    val locationManager = activity.getSystemService(Context.LOCATION_SERVICE) as LocationManager
    return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
      locationManager.isLocationEnabled
    } else {
      @Suppress("DEPRECATION")
      locationManager.isProviderEnabled(LocationManager.GPS_PROVIDER) ||
        locationManager.isProviderEnabled(LocationManager.NETWORK_PROVIDER)
    }
  }

  @SuppressLint("MissingPermission")
  @Suppress("DEPRECATION")
  private fun collectFreshScans(sampleCount: Int): JSObject {
    val samples = JSONArray()

    for (index in 0 until sampleCount) {
      val latch = CountDownLatch(1)
      var scanSucceeded = false

      val receiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
          scanSucceeded = intent.getBooleanExtra(WifiManager.EXTRA_RESULTS_UPDATED, false)
          latch.countDown()
        }
      }

      val filter = IntentFilter(WifiManager.SCAN_RESULTS_AVAILABLE_ACTION)
      registerReceiver(receiver, filter)

      val started = try {
        wifiManager.startScan()
      } catch (error: SecurityException) {
        unregisterReceiver(receiver)
        throw IllegalStateException("Android denied Wi-Fi scan access. Check permissions.", error)
      }

      if (!started) {
        unregisterReceiver(receiver)
        val throttle = if (scanThrottleEnabled() == true) {
          " Wi-Fi scan throttling is enabled in Developer Options."
        } else {
          ""
        }
        throw IllegalStateException("Android rejected the Wi-Fi scan request.$throttle")
      }

      val completed = latch.await(20, TimeUnit.SECONDS)
      unregisterReceiver(receiver)

      if (!completed) {
        throw IllegalStateException("Timed out waiting for Wi-Fi scan $index.")
      }

      if (!scanSucceeded) {
        throw IllegalStateException("Wi-Fi scan $index did not produce fresh results.")
      }

      samples.put(scanSample(index))
    }

    val response = JSObject()
    response.put("sampleCount", sampleCount)
    response.put("scanThrottleEnabled", scanThrottleEnabled() ?: JSONObject.NULL)
    response.put("samples", samples)
    return response
  }

  @SuppressLint("MissingPermission")
  @Suppress("DEPRECATION")
  private fun scanSample(index: Int): JSObject {
    val networks = JSONArray()
    for (result in wifiManager.scanResults) {
      val ssid = result.SSID ?: ""
      val bssid = result.BSSID ?: continue
      val network = JSObject()
      network.put("ssid", ssid)
      network.put("bssid", bssid)
      network.put("level", result.level)
      network.put("frequency", result.frequency)
      network.put("timestampMicros", result.timestamp)
      networks.put(network)
    }

    val sample = JSObject()
    sample.put("index", index)
    sample.put("networks", networks)
    return sample
  }

  private fun scanThrottleEnabled(): Boolean? {
    return try {
      WifiManager::class.java.getMethod("isScanThrottleEnabled").invoke(wifiManager) as? Boolean
    } catch (_: Exception) {
      null
    }
  }

  private fun registerReceiver(receiver: BroadcastReceiver, filter: IntentFilter) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      activity.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
    } else {
      @Suppress("DEPRECATION")
      activity.registerReceiver(receiver, filter)
    }
  }

  private fun unregisterReceiver(receiver: BroadcastReceiver) {
    try {
      activity.unregisterReceiver(receiver)
    } catch (_: IllegalArgumentException) {
    }
  }

  private fun copyToUri(source: File, destination: Uri): Long {
    var bytes = 0L
    source.inputStream().use { input ->
      activity.contentResolver.openOutputStream(destination, "wt").use { output ->
        if (output == null) {
          throw IllegalStateException("Could not open selected destination.")
        }

        val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
        while (true) {
          val read = input.read(buffer)
          if (read == -1) {
            break
          }
          output.write(buffer, 0, read)
          bytes += read.toLong()
        }
      }
    }
    return bytes
  }
}
