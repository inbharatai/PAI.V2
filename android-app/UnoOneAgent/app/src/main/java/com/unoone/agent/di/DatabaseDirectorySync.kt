package com.unoone.agent.di

import android.system.Os
import android.system.OsConstants
import java.io.File

internal object DatabaseDirectorySync {
    fun sync(directory: File) {
        require(directory.isDirectory)
        val fd = Os.open(directory.path, OsConstants.O_RDONLY, 0)
        try { Os.fsync(fd) } finally { Os.close(fd) }
    }
}
