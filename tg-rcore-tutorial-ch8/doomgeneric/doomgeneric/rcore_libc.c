/*
 * Minimal C library for Doom on rCore (RISC-V 64-bit bare-metal).
 * Provides stdio, stdlib, string, math stubs on top of rCore syscalls.
 */

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

void *memcpy(void *dst, const void *src, size_t n);
void *memset(void *s, int c, size_t n);

/* ---------- Syscall wrappers ---------- */

enum {
    SYS_close     = 57,
    SYS_openat    = 56,
    SYS_lseek     = 62,
    SYS_read      = 63,
    SYS_write     = 64,
    SYS_fstat     = 80,
    SYS_exit      = 93,
    SYS_brk       = 214,
};

static long sc1(long n, long a0)
{
    register long a7 __asm__("a7") = n;
    register long r0 __asm__("a0") = a0;
    __asm__ volatile("ecall" : "+r"(r0) : "r"(a7) : "memory");
    return r0;
}

static long sc2(long n, long a0, long a1)
{
    register long a7 __asm__("a7") = n;
    register long r0 __asm__("a0") = a0;
    register long r1 __asm__("a1") = a1;
    __asm__ volatile("ecall" : "+r"(r0) : "r"(r1), "r"(a7) : "memory");
    return r0;
}

static long sc3(long n, long a0, long a1, long a2)
{
    register long a7 __asm__("a7") = n;
    register long r0 __asm__("a0") = a0;
    register long r1 __asm__("a1") = a1;
    register long r2 __asm__("a2") = a2;
    __asm__ volatile("ecall" : "+r"(r0) : "r"(r1), "r"(r2), "r"(a7) : "memory");
    return r0;
}

/* ---------- errno ---------- */

int errno;

/* ---------- Heap (static 8 MiB) ---------- */

#define HEAP_SIZE (8 * 1024 * 1024)
static char heap_storage[HEAP_SIZE] __attribute__((aligned(16)));

typedef struct blk {
    size_t size;
    struct blk *next;
    int free;
} blk_t;

#define BLK_HDR sizeof(blk_t)
static blk_t *heap_head;
static int heap_inited;

static void heap_init(void)
{
    heap_head = (blk_t *)heap_storage;
    heap_head->size = HEAP_SIZE - BLK_HDR;
    heap_head->next = NULL;
    heap_head->free = 1;
    heap_inited = 1;
}

void *malloc(size_t size)
{
    if (!heap_inited) heap_init();
    if (size == 0) return NULL;
    size = (size + 15) & ~(size_t)15;

    blk_t *cur = heap_head;
    while (cur) {
        if (cur->free && cur->size >= size) {
            if (cur->size >= size + BLK_HDR + 16) {
                blk_t *nxt = (blk_t *)((char *)cur + BLK_HDR + size);
                nxt->size = cur->size - size - BLK_HDR;
                nxt->next = cur->next;
                nxt->free = 1;
                cur->size = size;
                cur->next = nxt;
            }
            cur->free = 0;
            return (char *)cur + BLK_HDR;
        }
        cur = cur->next;
    }
    return NULL;
}

void free(void *ptr)
{
    if (!ptr) return;
    blk_t *b = (blk_t *)((char *)ptr - BLK_HDR);
    b->free = 1;
    /* coalesce with next */
    if (b->next && b->next->free) {
        b->size += BLK_HDR + b->next->size;
        b->next = b->next->next;
    }
}

void *realloc(void *ptr, size_t size)
{
    if (!ptr) return malloc(size);
    if (size == 0) { free(ptr); return NULL; }
    blk_t *b = (blk_t *)((char *)ptr - BLK_HDR);
    if (b->size >= size) return ptr;
    void *np = malloc(size);
    if (!np) return NULL;
    memcpy(np, ptr, b->size);
    free(ptr);
    return np;
}

void *calloc(size_t n, size_t size)
{
    size_t total = n * size;
    void *p = malloc(total);
    if (p) memset(p, 0, total);
    return p;
}

/* ---------- String functions ---------- */

void *memcpy(void *dst, const void *src, size_t n)
{
    unsigned char *d = dst;
    const unsigned char *s = src;
    while (n--) *d++ = *s++;
    return dst;
}

void *memset(void *s, int c, size_t n)
{
    unsigned char *p = s;
    while (n--) *p++ = (unsigned char)c;
    return s;
}

void *memmove(void *dst, const void *src, size_t n)
{
    unsigned char *d = dst;
    const unsigned char *s = src;
    if (d < s) {
        while (n--) *d++ = *s++;
    } else {
        d += n; s += n;
        while (n--) *--d = *--s;
    }
    return dst;
}

int memcmp(const void *s1, const void *s2, size_t n)
{
    const unsigned char *a = s1, *b = s2;
    while (n--) {
        if (*a != *b) return *a - *b;
        a++; b++;
    }
    return 0;
}

size_t strlen(const char *s)
{
    const char *p = s;
    while (*p) p++;
    return (size_t)(p - s);
}

char *strcpy(char *dst, const char *src)
{
    char *d = dst;
    while ((*d++ = *src++));
    return dst;
}

char *strncpy(char *dst, const char *src, size_t n)
{
    char *d = dst;
    while (n && *src) {
        *d++ = *src++;
        n--;
    }
    while (n--) *d++ = '\0';
    return dst;
}

int strcmp(const char *a, const char *b)
{
    while (*a && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}

int strncmp(const char *a, const char *b, size_t n)
{
    if (!n) return 0;
    while (--n && *a && *a == *b) { a++; b++; }
    return (unsigned char)*a - (unsigned char)*b;
}

int strcasecmp(const char *a, const char *b)
{
    for (;;) {
        int ca = (unsigned char)*a++;
        int cb = (unsigned char)*b++;
        if (ca >= 'A' && ca <= 'Z') ca += 32;
        if (cb >= 'A' && cb <= 'Z') cb += 32;
        if (ca != cb) return ca - cb;
        if (ca == 0) return 0;
    }
}

int strncasecmp(const char *a, const char *b, size_t n)
{
    for (size_t i = 0; i < n; i++) {
        int ca = (unsigned char)a[i];
        int cb = (unsigned char)b[i];
        if (ca >= 'A' && ca <= 'Z') ca += 32;
        if (cb >= 'A' && cb <= 'Z') cb += 32;
        if (ca != cb) return ca - cb;
        if (ca == 0) return 0;
    }
    return 0;
}

char *strcat(char *dst, const char *src)
{
    char *d = dst;
    while (*d) d++;
    while ((*d++ = *src++));
    return dst;
}

char *strncat(char *dst, const char *src, size_t n)
{
    char *d = dst;
    while (*d) d++;
    while (n-- && (*d = *src++)) d++;
    *d = '\0';
    return dst;
}

char *strchr(const char *s, int c)
{
    char ch = (char)c;
    while (*s) {
        if (*s == ch) return (char *)s;
        s++;
    }
    return ch == '\0' ? (char *)s : NULL;
}

char *strrchr(const char *s, int c)
{
    char ch = (char)c;
    const char *last = NULL;
    while (*s) {
        if (*s == ch) last = s;
        s++;
    }
    if (ch == '\0') return (char *)s;
    return (char *)last;
}

char *strstr(const char *h, const char *n)
{
    if (!*n) return (char *)h;
    for (; *h; h++) {
        const char *a = h, *b = n;
        while (*a && *b && *a == *b) { a++; b++; }
        if (!*b) return (char *)h;
    }
    return NULL;
}

char *strdup(const char *s)
{
    size_t l = strlen(s) + 1;
    char *d = malloc(l);
    if (d) memcpy(d, s, l);
    return d;
}

static char *strtok_save;
char *strtok(char *str, const char *delim)
{
    if (!str) str = strtok_save;
    if (!str) return NULL;
    while (*str && strchr(delim, *str)) str++;
    if (!*str) { strtok_save = NULL; return NULL; }
    char *tok = str;
    while (*str && !strchr(delim, *str)) str++;
    if (*str) { *str = '\0'; str++; }
    strtok_save = str;
    return tok;
}

/* ---------- ctype ---------- */

int isalpha(int c) { return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z'); }
int isdigit(int c) { return c >= '0' && c <= '9'; }
int isalnum(int c) { return isalpha(c) || isdigit(c); }
int isspace(int c) { return c == ' ' || c == '\t' || c == '\n' || c == '\r' || c == '\f' || c == '\v'; }
int isupper(int c) { return c >= 'A' && c <= 'Z'; }
int islower(int c) { return c >= 'a' && c <= 'z'; }
int isprint(int c) { return c >= 0x20 && c <= 0x7e; }
int isxdigit(int c) { return isdigit(c) || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F'); }
int toupper(int c) { return (c >= 'a' && c <= 'z') ? c - 32 : c; }
int tolower(int c) { return (c >= 'A' && c <= 'Z') ? c + 32 : c; }

/* ---------- stdlib misc ---------- */

int abs(int x) { return x < 0 ? -x : x; }
long labs(long x) { return x < 0 ? -x : x; }

int atoi(const char *s)
{
    int neg = 0, v = 0;
    while (isspace((unsigned char)*s)) s++;
    if (*s == '-') { neg = 1; s++; }
    else if (*s == '+') s++;
    while (isdigit((unsigned char)*s)) { v = v * 10 + (*s - '0'); s++; }
    return neg ? -v : v;
}

long atol(const char *s) { return (long)atoi(s); }

double atof(const char *s)
{
    double v = 0.0, frac = 0.0, div = 1.0;
    int neg = 0;
    while (isspace((unsigned char)*s)) s++;
    if (*s == '-') { neg = 1; s++; }
    else if (*s == '+') s++;
    while (isdigit((unsigned char)*s)) { v = v * 10.0 + (*s - '0'); s++; }
    if (*s == '.') {
        s++;
        while (isdigit((unsigned char)*s)) { frac = frac * 10.0 + (*s - '0'); div *= 10.0; s++; }
    }
    v += frac / div;
    return neg ? -v : v;
}

void qsort(void *base, size_t nmemb, size_t size,
           int (*cmp)(const void *, const void *))
{
    /* Simple insertion sort – Doom calls qsort on tiny arrays only. */
    char *b = base;
    char tmp[256];
    for (size_t i = 1; i < nmemb; i++) {
        size_t j = i;
        while (j > 0 && cmp(b + j * size, b + (j - 1) * size) < 0) {
            memcpy(tmp, b + j * size, size);
            memcpy(b + j * size, b + (j - 1) * size, size);
            memcpy(b + (j - 1) * size, tmp, size);
            j--;
        }
    }
}

void exit(int status) { sc1(SYS_exit, status); __builtin_unreachable(); }
void abort(void) { exit(127); }
void __assert_fail(const char *expr, const char *file, int line)
{
    (void)expr; (void)file; (void)line;
    abort();
}

int system(const char *cmd) { (void)cmd; return -1; }
char *getenv(const char *name) { (void)name; return NULL; }

/* ---------- FILE I/O ---------- */

#define MAX_FILES 32

typedef struct _FILE {
    int fd;
    int eof;
    int err;
    int used;
} FILE;

static FILE file_table[MAX_FILES];
static FILE stdin_f  = { .fd = 0, .used = 1 };
static FILE stdout_f = { .fd = 1, .used = 1 };
static FILE stderr_f = { .fd = 2, .used = 1 };

FILE *stdin  = &stdin_f;
FILE *stdout = &stdout_f;
FILE *stderr = &stderr_f;

static FILE *alloc_file(int fd)
{
    for (int i = 0; i < MAX_FILES; i++) {
        if (!file_table[i].used) {
            file_table[i].fd   = fd;
            file_table[i].eof  = 0;
            file_table[i].err  = 0;
            file_table[i].used = 1;
            return &file_table[i];
        }
    }
    return NULL;
}

FILE *fopen(const char *path, const char *mode)
{
    int flags = 0;
    if (mode[0] == 'r') flags = 0;        /* O_RDONLY */
    else if (mode[0] == 'w') flags = 0x601; /* O_WRONLY|O_CREAT|O_TRUNC */
    else if (mode[0] == 'a') flags = 0x201; /* O_WRONLY|O_CREAT */
    /* NOTE: the kernel openat dispatch uses (path, flags) directly,
       not Linux's (dirfd, path, flags). Our Rust user wrapper matches. */
    int fd = (int)sc2(SYS_openat, (long)path, (long)flags);
    if (fd < 0) return NULL;
    return alloc_file(fd);
}

int fclose(FILE *f)
{
    if (!f || !f->used) return -1;
    int r = (int)sc1(SYS_close, f->fd);
    f->used = 0;
    return r;
}

size_t fread(void *ptr, size_t size, size_t nmemb, FILE *f)
{
    if (!f || !size || !nmemb) return 0;
    size_t total = size * nmemb;
    long r = sc3(SYS_read, f->fd, (long)ptr, (long)total);
    if (r <= 0) { f->eof = 1; return 0; }
    return (size_t)r / size;
}

size_t fwrite(const void *ptr, size_t size, size_t nmemb, FILE *f)
{
    if (!f || !size || !nmemb) return 0;
    size_t total = size * nmemb;
    long r = sc3(SYS_write, f->fd, (long)ptr, (long)total);
    if (r < 0) { f->err = 1; return 0; }
    return (size_t)r / size;
}

int fseek(FILE *f, long offset, int whence)
{
    if (!f) return -1;
    long r = sc3(SYS_lseek, f->fd, offset, whence);
    if (r < 0) return -1;
    f->eof = 0;
    return 0;
}

long ftell(FILE *f)
{
    if (!f) return -1;
    return (long)sc3(SYS_lseek, f->fd, 0, 1 /* SEEK_CUR */);
}

int fflush(FILE *f) { (void)f; return 0; }
int feof(FILE *f) { return f ? f->eof : 1; }
int ferror(FILE *f) { return f ? f->err : 1; }
int fileno(FILE *f) { return f ? f->fd : -1; }
void rewind(FILE *f) { if (f) { fseek(f, 0, 0); f->eof = 0; f->err = 0; } }
void clearerr(FILE *f) { if (f) { f->eof = 0; f->err = 0; } }

int fgetc(FILE *f)
{
    unsigned char c;
    if (fread(&c, 1, 1, f) != 1) return -1;
    return c;
}

int fputc(int c, FILE *f)
{
    unsigned char ch = (unsigned char)c;
    if (fwrite(&ch, 1, 1, f) != 1) return -1;
    return ch;
}

char *fgets(char *s, int size, FILE *f)
{
    if (size <= 0) return NULL;
    int i = 0;
    while (i < size - 1) {
        int c = fgetc(f);
        if (c == -1) { if (i == 0) return NULL; break; }
        s[i++] = (char)c;
        if (c == '\n') break;
    }
    s[i] = '\0';
    return s;
}

int fputs(const char *s, FILE *f)
{
    size_t l = strlen(s);
    return fwrite(s, 1, l, f) == l ? (int)l : -1;
}

int remove(const char *path) { (void)path; return -1; }
int rename(const char *old, const char *new_name) { (void)old; (void)new_name; return -1; }

/* ---------- printf engine ---------- */

static void out_char(char **buf, size_t *pos, size_t max, FILE *fp, int c)
{
    if (fp) {
        unsigned char ch = (unsigned char)c;
        sc3(SYS_write, fp->fd, (long)&ch, 1);
    }
    if (buf && *pos < max) (*buf)[*pos] = (char)c;
    (*pos)++;
}

static void out_str(char **buf, size_t *pos, size_t max, FILE *fp, const char *s)
{
    while (*s) out_char(buf, pos, max, fp, *s++);
}

static void out_uint(char **buf, size_t *pos, size_t max, FILE *fp,
                     unsigned long long v, int base, int width, char pad, int upper)
{
    char tmp[24];
    const char *digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    int i = 0;
    if (v == 0) tmp[i++] = '0';
    while (v) { tmp[i++] = digits[v % base]; v /= base; }
    while (i < width) tmp[i++] = pad;
    while (i > 0) out_char(buf, pos, max, fp, tmp[--i]);
}

static int do_printf(char **buf, size_t max, FILE *fp, const char *fmt, va_list ap)
{
    size_t pos = 0;
    for (; *fmt; fmt++) {
        if (*fmt != '%') { out_char(buf, &pos, max, fp, *fmt); continue; }
        fmt++;
        char pad = ' ';
        int width = 0, precision = -1, long_flag = 0;
        if (*fmt == '0') { pad = '0'; fmt++; }
        if (*fmt == '-') { fmt++; } /* left-align: ignored in this simple impl */
        while (*fmt >= '0' && *fmt <= '9') { width = width * 10 + (*fmt - '0'); fmt++; }
        if (*fmt == '.') {
            precision = 0;
            fmt++;
            while (*fmt >= '0' && *fmt <= '9') {
                precision = precision * 10 + (*fmt - '0');
                fmt++;
            }
        }
        if (*fmt == 'l') { long_flag++; fmt++; }
        if (*fmt == 'l') { long_flag++; fmt++; }
        switch (*fmt) {
        case 'd': case 'i': {
            long long v = long_flag >= 2 ? va_arg(ap, long long)
                        : long_flag ? (long long)va_arg(ap, long)
                        : (long long)va_arg(ap, int);
            if (v < 0) { out_char(buf, &pos, max, fp, '-'); v = -v; }
            if (precision > width) width = precision;
            out_uint(buf, &pos, max, fp, (unsigned long long)v, 10, width, precision >= 0 ? '0' : pad, 0);
            break;
        }
        case 'u': {
            unsigned long long v = long_flag >= 2 ? va_arg(ap, unsigned long long)
                                 : long_flag ? (unsigned long long)va_arg(ap, unsigned long)
                                 : (unsigned long long)va_arg(ap, unsigned int);
            if (precision > width) width = precision;
            out_uint(buf, &pos, max, fp, v, 10, width, precision >= 0 ? '0' : pad, 0);
            break;
        }
        case 'x': case 'X': {
            unsigned long long v = long_flag >= 2 ? va_arg(ap, unsigned long long)
                                 : long_flag ? (unsigned long long)va_arg(ap, unsigned long)
                                 : (unsigned long long)va_arg(ap, unsigned int);
            if (precision > width) width = precision;
            out_uint(buf, &pos, max, fp, v, 16, width, precision >= 0 ? '0' : pad, *fmt == 'X');
            break;
        }
        case 'p': {
            unsigned long long v = (unsigned long long)(uintptr_t)va_arg(ap, void *);
            out_str(buf, &pos, max, fp, "0x");
            out_uint(buf, &pos, max, fp, v, 16, 0, '0', 0);
            break;
        }
        case 'c':
            out_char(buf, &pos, max, fp, va_arg(ap, int));
            break;
        case 's': {
            const char *s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            int len = (int)strlen(s);
            while (len < width) { out_char(buf, &pos, max, fp, ' '); width--; }
            out_str(buf, &pos, max, fp, s);
            break;
        }
        case '%':
            out_char(buf, &pos, max, fp, '%');
            break;
        default:
            out_char(buf, &pos, max, fp, '%');
            out_char(buf, &pos, max, fp, *fmt);
            break;
        }
    }
    if (buf && pos < max) (*buf)[pos] = '\0';
    else if (buf && max > 0) (*buf)[max - 1] = '\0';
    return (int)pos;
}

int vfprintf(FILE *f, const char *fmt, va_list ap)
{
    return do_printf(NULL, 0, f, fmt, ap);
}

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap)
{
    return do_printf(&buf, size, NULL, fmt, ap);
}

int fprintf(FILE *f, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt); int r = vfprintf(f, fmt, ap); va_end(ap); return r;
}

int printf(const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt); int r = vfprintf(stdout, fmt, ap); va_end(ap); return r;
}

int sprintf(char *buf, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt); int r = vsnprintf(buf, (size_t)-1, fmt, ap); va_end(ap); return r;
}

int snprintf(char *buf, size_t size, const char *fmt, ...)
{
    va_list ap; va_start(ap, fmt); int r = vsnprintf(buf, size, fmt, ap); va_end(ap); return r;
}

int puts(const char *s)
{
    int r = fputs(s, stdout);
    fputc('\n', stdout);
    return r >= 0 ? r + 1 : -1;
}

int putchar(int c) { return fputc(c, stdout); }
int getchar(void) { return fgetc(stdin); }

/* ---------- math ---------- */

double fabs(double x) { return x < 0 ? -x : x; }

double floor(double x)
{
    long long i = (long long)x;
    if (x < 0 && x != (double)i) return (double)(i - 1);
    return (double)i;
}

double ceil(double x)
{
    long long i = (long long)x;
    if (x > 0 && x != (double)i) return (double)(i + 1);
    return (double)i;
}

double sqrt(double x)
{
    double r;
    __asm__ ("fsqrt.d %0, %1" : "=f"(r) : "f"(x));
    return r;
}

/* Minimax polynomial for atan on [0, 1]. */
static double atan_approx(double x)
{
    double x2 = x * x;
    return x * (1.0 - x2 * (1.0/3.0 - x2 * (1.0/5.0 - x2 * (1.0/7.0
               - x2 * (1.0/9.0 - x2 * (1.0/11.0 - x2 / 13.0))))));
}

double atan(double x)
{
    static const double PI_2 = 1.5707963267948966;
    if (x < 0) return -atan(-x);
    if (x > 1.0) return PI_2 - atan_approx(1.0 / x);
    return atan_approx(x);
}

double atan2(double y, double x)
{
    static const double PI   = 3.14159265358979323846;
    static const double PI_2 = 1.5707963267948966;
    if (x > 0)             return atan(y / x);
    if (x < 0 && y >= 0)   return atan(y / x) + PI;
    if (x < 0 && y < 0)    return atan(y / x) - PI;
    if (y > 0)              return PI_2;
    if (y < 0)              return -PI_2;
    return 0.0;
}

double pow(double base, double exp)
{
    if (exp == 0.0) return 1.0;
    if (base == 0.0) return 0.0;
    /* integer exponent fast-path */
    int neg = 0;
    if (exp < 0) { neg = 1; exp = -exp; }
    long long ei = (long long)exp;
    if ((double)ei == exp) {
        double r = 1.0;
        double b = base;
        unsigned long long e = (unsigned long long)ei;
        while (e) {
            if (e & 1) r *= b;
            b *= b;
            e >>= 1;
        }
        return neg ? 1.0 / r : r;
    }
    /* fallback: exp(exp * ln(base)) via iterative */
    return 0.0; /* Doom only uses integer exponents */
}

double log(double x) { (void)x; return 0.0; }

/* ---------- Misc stubs ---------- */

int isatty(int fd) { return fd < 3; }
int access(const char *path, int mode) { (void)path; (void)mode; return -1; }

int stat(const char *path, void *buf) { (void)path; (void)buf; return -1; }
int fstat(int fd, void *buf)
{
    (void)fd; (void)buf;
    return 0;
}
int mkdir(const char *path, unsigned long mode) { (void)path; (void)mode; return -1; }

typedef long time_t;
time_t time(time_t *t) { if (t) *t = 0; return 0; }

/* atexit */
#define MAX_ATEXIT 32
static void (*atexit_fns[MAX_ATEXIT])(void);
static int atexit_count;

int atexit(void (*fn)(void))
{
    if (atexit_count >= MAX_ATEXIT) return -1;
    atexit_fns[atexit_count++] = fn;
    return 0;
}

/* sscanf minimal stub – Doom uses it once or twice */
int sscanf(const char *str, const char *fmt, ...)
{
    (void)str; (void)fmt;
    return 0;
}
