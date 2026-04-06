#ifndef _RCORE_SYS_STAT_H
#define _RCORE_SYS_STAT_H

#include <sys/types.h>

struct stat {
    unsigned long st_dev;
    unsigned long st_ino;
    mode_t st_mode;
    unsigned long st_nlink;
    unsigned long st_uid;
    unsigned long st_gid;
    unsigned long st_rdev;
    off_t st_size;
    long st_blksize;
    long st_blocks;
    long st_atime;
    long st_mtime;
    long st_ctime;
};

int stat(const char *path, struct stat *buf);
int fstat(int fd, struct stat *buf);
int mkdir(const char *path, mode_t mode);

#define S_IFMT 0170000
#define S_IFDIR 0040000
#define S_IFREG 0100000
#define S_IFCHR 0020000
#define S_ISDIR(m) (((m) & S_IFMT) == S_IFDIR)
#define S_ISREG(m) (((m) & S_IFMT) == S_IFREG)

#endif /* _RCORE_SYS_STAT_H */
