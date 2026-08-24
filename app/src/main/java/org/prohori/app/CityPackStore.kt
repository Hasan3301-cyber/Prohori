package org.prohori.app

import android.content.Context
import android.net.Uri
import org.prohori.core.CityPackFile
import org.prohori.core.CityPackInstall
import org.prohori.core.Prohori
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.util.zip.ZipInputStream

/** Loads the bundled, signed P3 demonstration pack without network or storage permission. */
class CityPackStore(private val context: Context) {
    private val directory = File(context.filesDir, "city-packs")
    private val activeArchive = File(directory, "active.prohori-pack")
    private val backupArchive = File(directory, "active.previous")

    fun installActiveOrBundled(core: Prohori): CityPackInstall =
        if (activeArchive.isFile) {
            runCatching { installArchive(core, activeArchive) }
                .getOrElse {
                    activeArchive.delete()
                    installBundledDemo(core)
                }
        } else {
            installBundledDemo(core)
        }

    fun import(uri: Uri, core: Prohori): CityPackInstall {
        directory.mkdirs()
        val temporary = File(directory, "active.importing")
        var total = 0L
        context.contentResolver.openInputStream(uri).use { input ->
            requireNotNull(input) { "The selected pack could not be opened" }
            FileOutputStream(temporary, false).use { output ->
                val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    total += read
                    require(total <= MAX_ARCHIVE_BYTES) { "The city pack is larger than 100 MB" }
                    output.write(buffer, 0, read)
                }
                output.fd.sync()
            }
        }
        return try {
            val installed = installArchive(core, temporary)
            if (installed.accepted) {
                promoteVerifiedArchive(temporary)
            } else {
                temporary.delete()
            }
            installed
        } catch (error: Throwable) {
            temporary.delete()
            throw error
        }
    }

    private fun promoteVerifiedArchive(temporary: File) {
        if (backupArchive.exists() && !backupArchive.delete()) {
            error("Could not prepare city-pack rollback storage")
        }
        val hadActive = activeArchive.isFile
        if (hadActive && !activeArchive.renameTo(backupArchive)) {
            error("Could not preserve the previous city pack")
        }
        if (!temporary.renameTo(activeArchive)) {
            val restored = !hadActive || backupArchive.renameTo(activeArchive)
            check(restored) { "City-pack update failed and the previous archive could not be restored" }
            error("Could not finish installing the city pack; the previous pack was restored")
        }
        // The active file is already durable; a stale rollback file is harmless if cleanup fails.
        backupArchive.delete()
    }

    private fun installBundledDemo(core: Prohori): CityPackInstall {
        val root = "city-pack/ruet-demo"
        val manifest = context.assets.open("$root/manifest.json").use { it.readBytes() }
        val key = trustedKey()
        val files =
            PAYLOADS.map { path ->
                CityPackFile(
                    path = path,
                    bytes = context.assets.open("$root/$path").use { it.readBytes() },
                )
            }
        return core.installCityPack(manifest, files, key)
    }

    private fun installArchive(core: Prohori, archive: File): CityPackInstall {
        val entries = linkedMapOf<String, ByteArray>()
        var expanded = 0L
        ZipInputStream(FileInputStream(archive)).use { zip ->
            while (true) {
                val entry = zip.nextEntry ?: break
                require(!entry.isDirectory) { "City-pack directories are not allowed" }
                val name = entry.name
                require(name == "manifest.json" || name in PAYLOADS) {
                    "Unexpected city-pack entry: $name"
                }
                require('/' !in name && '\\' !in name && name != "..") {
                    "Unsafe city-pack entry: $name"
                }
                require(!entries.containsKey(name)) { "Duplicate city-pack entry: $name" }
                val bytes = zip.readBounded(MAX_ENTRY_BYTES)
                expanded += bytes.size
                require(expanded <= MAX_EXPANDED_BYTES) { "Expanded city pack is larger than 100 MB" }
                entries[name] = bytes
                zip.closeEntry()
            }
        }
        val manifest = requireNotNull(entries.remove("manifest.json")) { "manifest.json is missing" }
        require(entries.keys == PAYLOADS.toSet()) { "City pack is missing a required payload" }
        val files = entries.map { (path, bytes) -> CityPackFile(path, bytes) }
        return core.installCityPack(manifest, files, trustedKey())
    }

    private fun ZipInputStream.readBounded(maximum: Int): ByteArray {
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
        while (true) {
            val read = read(buffer)
            if (read < 0) break
            require(output.size() + read <= maximum) { "A city-pack entry is larger than 50 MB" }
            output.write(buffer, 0, read)
        }
        return output.toByteArray()
    }

    private fun trustedKey(): ByteArray =
        context.assets
            .open("city-pack/ruet-demo/verification-key.hex")
            .bufferedReader()
            .use { it.readText().trim() }
            .hexToBytes()

    private fun String.hexToBytes(): ByteArray {
        require(length == 64 && all { it.isDigit() || it.lowercaseChar() in 'a'..'f' }) {
            "The bundled city-pack verification key is invalid"
        }
        return chunked(2).map { it.toInt(16).toByte() }.toByteArray()
    }

    private companion object {
        const val MAX_ARCHIVE_BYTES = 100_000_000L
        const val MAX_EXPANDED_BYTES = 100_000_000L
        const val MAX_ENTRY_BYTES = 50_000_000
        val PAYLOADS =
            listOf(
                "conditions.snap",
                "emergency.json",
                "hospitals.json",
                "roads.graph",
                "shelters.json",
                "zones.geojson",
            )
    }
}
