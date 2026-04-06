#ifndef _RCORE_STDLIB_H
#define _RCORE_STDLIB_H

#include <stddef.h>

void *malloc(size_t size);
void free(void *ptr);
void *realloc(void *ptr, size_t size);
void *calloc(size_t n, size_t size);

void exit(int status);
void abort(void);

int atoi(const char *s);
long atol(const char *s);
double atof(const char *s);

int abs(int x);
long labs(long x);

int system(const char *cmd);
char *getenv(const char *name);

void qsort(void *base, size_t nmemb, size_t size,
           int (*cmp)(const void *, const void *));

#endif /* _RCORE_STDLIB_H */
