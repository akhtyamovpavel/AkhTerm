package org.akhterm.mobile.data.storage

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import kotlinx.serialization.encodeToString
import org.akhterm.mobile.data.model.PairedCredentials
import org.akhterm.mobile.data.model.RemoteJson
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Encrypts paired credentials ({baseUrl, deviceId, token, hostName}) using
 * Android Keystore AES-256-GCM with per-write random 96-bit IVs.
 */
class SecureTokenStore(
    context: Context,
) {
    private val prefs = context.applicationContext.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    @Synchronized
    fun load(): PairedCredentials? {
        val encoded = prefs.getString(KEY_PAYLOAD, null) ?: return null
        return try {
            val blob = Base64.decode(encoded, Base64.NO_WRAP)
            if (blob.size <= GCM_IV_LENGTH) {
                clear()
                return null
            }
            val iv = blob.copyOfRange(0, GCM_IV_LENGTH)
            val ciphertext = blob.copyOfRange(GCM_IV_LENGTH, blob.size)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, getOrCreateSecretKey(), GCMParameterSpec(GCM_TAG_BITS, iv))
            val plaintext = cipher.doFinal(ciphertext).toString(Charsets.UTF_8)
            RemoteJson.decodeFromString(PairedCredentials.serializer(), plaintext)
        } catch (_: Exception) {
            clear()
            null
        }
    }

    @Synchronized
    fun save(credentials: PairedCredentials) {
        val plaintext = RemoteJson.encodeToString(credentials).toByteArray(Charsets.UTF_8)
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, getOrCreateSecretKey())
        val iv = cipher.iv
        val ciphertext = cipher.doFinal(plaintext)
        val combined = ByteArray(iv.size + ciphertext.size)
        System.arraycopy(iv, 0, combined, 0, iv.size)
        System.arraycopy(ciphertext, 0, combined, iv.size, ciphertext.size)
        val encoded = Base64.encodeToString(combined, Base64.NO_WRAP)
        prefs.edit().putString(KEY_PAYLOAD, encoded).apply()
    }

    @Synchronized
    fun clear() {
        prefs.edit().remove(KEY_PAYLOAD).apply()
    }

    private fun getOrCreateSecretKey(): SecretKey {
        val keyStore = KeyStore.getInstance(ANDROID_KEYSTORE).apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }

        val keyGenerator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, ANDROID_KEYSTORE)
        val spec = KeyGenParameterSpec.Builder(
            KEY_ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build()
        keyGenerator.init(spec)
        return keyGenerator.generateKey()
    }

    companion object {
        private const val ANDROID_KEYSTORE = "AndroidKeyStore"
        private const val KEY_ALIAS = "akhterm_mobile_credentials_v1"
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private const val GCM_IV_LENGTH = 12
        private const val GCM_TAG_BITS = 128
        private const val PREFS_NAME = "akhterm_secure_store"
        private const val KEY_PAYLOAD = "paired_credentials_gcm"
    }
}
