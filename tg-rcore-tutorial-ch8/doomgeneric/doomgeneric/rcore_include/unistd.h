#ifndef _RCORE_UNISTD_H
#define _RCORE_UNISTD_H

#ifndef _RCORE_SSIZE_T_DEFINED
#define _RCORE_SSIZE_T_DEFINED
typedef long ssize_t;
#endif

#define R_OK 4
#define W_OK 2
#define F_OK 0

int isatty(int fd);
int access(const char *path, int mode);

#endif /* _RCORE_UNISTD_H */
