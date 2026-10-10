package com.unoone.agent.execution

import android.system.Os
import android.system.OsConstants
import android.system.StructStat
import org.robolectric.annotation.Implementation
import org.robolectric.annotation.Implements
import org.robolectric.annotation.Resetter
import java.io.FileDescriptor
import java.nio.channels.FileChannel
import java.nio.file.Files
import java.nio.file.Path
import java.nio.file.StandardOpenOption

/** Robolectric ShadowLinux cannot open directories (uses RandomAccessFile). This host-only
 * adapter performs REAL host directory open/force/close for the unchanged native task WAL.
 * It does not stub away journal writes, validation, readback, admission or authorization. */
@Implements(Os::class)
class CalendarHostOsShadow {
    companion object {
        private val directories = mutableMapOf<FileDescriptor, FileChannel>()
        @JvmStatic @Implementation fun open(path: String, flags: Int, mode: Int): FileDescriptor {
            require(flags == OsConstants.O_RDONLY && mode == 0 && Files.isDirectory(Path.of(path)))
            val channel = FileChannel.open(Path.of(path),StandardOpenOption.READ)
            return FileDescriptor().also { directories[it] = channel }
        }
        @JvmStatic @Implementation fun fstat(fd: FileDescriptor): StructStat {
            check(directories.getValue(fd).isOpen)
            return StructStat(0,0,OsConstants.S_IFDIR,1,0,0,0,0,0,0,0,4096,0)
        }
        @JvmStatic @Implementation fun fsync(fd: FileDescriptor) { directories.getValue(fd).force(true) }
        @JvmStatic @Implementation fun close(fd: FileDescriptor) { checkNotNull(directories.remove(fd)).close() }
        @JvmStatic @Resetter fun reset() { directories.values.forEach { it.close() }; directories.clear() }
    }
}
