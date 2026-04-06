/*
 * rCore platform layer for doomgeneric.
 *
 * Syscall convention matches the Rust kernel dispatcher in ch8:
 *   openat(path, flags)  — NOT Linux's openat(dirfd, path, flags, mode)
 */

#include "doomgeneric.h"
#include "doomkeys.h"

#include <stdint.h>
#include <stdio.h>
#include <string.h>

enum {
	SYS_clock_gettime = 113,
	SYS_sched_yield   = 124,
	SYS_fb_get_info   = 1040,
	SYS_fb_present    = 1041,
	SYS_input_getchar = 1042,
	SYS_hart_snapshot = 1044,
	CLOCK_MONOTONIC   = 1,
};

#define KEYQUEUE_SIZE 64
#define DG_SMP_HART_CAPACITY 8

#ifndef DG_RCORE_INTERACTIVE
#define DG_RCORE_INTERACTIVE 1
#endif

static unsigned short s_key_queue[KEYQUEUE_SIZE];
static unsigned int s_key_w = 0;
static unsigned int s_key_r = 0;
static unsigned char s_release_queue[KEYQUEUE_SIZE];
static unsigned int s_release_w = 0;
static unsigned int s_release_r = 0;
static int s_escape_state = 0;
static int s_fb_present_error_logs = 0;
static int s_poll_armed = 1;

typedef struct {
	unsigned long tv_sec;
	unsigned long tv_nsec;
} rcore_timespec;

typedef struct {
	uintptr_t max_harts;
	uintptr_t online_harts;
	uintptr_t online_mask;
	uintptr_t current_hart;
	uintptr_t need_resched_mask;
	uint64_t timer_ticks[DG_SMP_HART_CAPACITY];
	uint64_t kernel_timer_interrupts[DG_SMP_HART_CAPACITY];
} rcore_hart_snapshot;

static long syscall0(long n)
{
	register long a7 __asm__("a7") = n;
	register long a0 __asm__("a0");
	__asm__ volatile("ecall" : "=r"(a0) : "r"(a7) : "memory");
	return a0;
}

static long syscall2(long n, long a0, long a1)
{
	register long a7 __asm__("a7") = n;
	register long a0_r __asm__("a0") = a0;
	register long a1_r __asm__("a1") = a1;
	__asm__ volatile("ecall"
			 : "+r"(a0_r)
			 : "r"(a1_r), "r"(a7)
			 : "memory");
	return a0_r;
}

static void queue_key(int pressed, unsigned char key)
{
	s_key_queue[s_key_w] = (unsigned short)((pressed << 8) | key);
	s_key_w = (s_key_w + 1) % KEYQUEUE_SIZE;
	if (s_key_w == s_key_r)
		s_key_r = (s_key_r + 1) % KEYQUEUE_SIZE;
}

static void queue_key_release(unsigned char key)
{
	s_release_queue[s_release_w] = key;
	s_release_w = (s_release_w + 1) % KEYQUEUE_SIZE;
	if (s_release_w == s_release_r)
		s_release_r = (s_release_r + 1) % KEYQUEUE_SIZE;
}

static void queue_key_tap(unsigned char key)
{
	/*
	 * 串口终端只能可靠地告诉我们“用户敲下了一个字符”，
	 * 很难像图形后端那样精确区分 keydown / keyup。
	 *
	 * 如果在同一轮直接塞入 keydown + keyup，Doom 会在构造本 tic 的
	 * movement command 之前把 gamekeydown 重新清零，结果看起来就是
	 * “按了方向键但角色完全不动”。
	 *
	 * 这里把 keyup 延后一轮 `DG_GetKey()` 轮询再发送，让按键至少在一个
	 * tic 内保持按下状态；若用户持续按住，终端的自动重复字符会继续产生
	 * 新的 keydown，于是移动会连续进行。
	 */
	queue_key(1, key);
	queue_key_release(key);
}

static void flush_pending_releases(void)
{
	while (s_release_r != s_release_w) {
		unsigned char key = s_release_queue[s_release_r];
		s_release_r = (s_release_r + 1) % KEYQUEUE_SIZE;
		queue_key(0, key);
	}
}

static unsigned char ascii_lower(unsigned char c)
{
	if (c >= 'A' && c <= 'Z')
		return (unsigned char)(c - 'A' + 'a');
	return c;
}

static void handle_escape_final(unsigned char c)
{
	switch (c) {
	case 'A': queue_key_tap(KEY_UPARROW); break;
	case 'B': queue_key_tap(KEY_DOWNARROW); break;
	case 'C': queue_key_tap(KEY_RIGHTARROW); break;
	case 'D': queue_key_tap(KEY_LEFTARROW); break;
	default: queue_key_tap(KEY_ESCAPE); break;
	}
}

static void handle_input_byte(unsigned char c)
{
	if (s_escape_state == 1) {
		if (c == '[') {
			s_escape_state = 2;
			return;
		}
		s_escape_state = 0;
		queue_key_tap(KEY_ESCAPE);
	}

	if (s_escape_state == 2) {
		s_escape_state = 0;
		handle_escape_final(c);
		return;
	}

	if (c == 0x1b) {
		s_escape_state = 1;
		return;
	}

	switch (c) {
	case '\r':
	case '\n':
		queue_key_tap(KEY_ENTER);
		break;
	case '\t':
		queue_key_tap(KEY_TAB);
		break;
	case ' ':
		queue_key_tap(KEY_USE);
		break;
	case '=':
	case '+':
		queue_key_tap(KEY_EQUALS);
		break;
	case '-':
	case '_':
		queue_key_tap(KEY_MINUS);
		break;
	case 0x7f:
		queue_key_tap(KEY_BACKSPACE);
		break;
	default: {
		unsigned char k = ascii_lower(c);
		switch (k) {
		case 'w': queue_key_tap(KEY_UPARROW); break;
		case 's': queue_key_tap(KEY_DOWNARROW); break;
		case 'a': queue_key_tap(KEY_LEFTARROW); break;
		case 'd': queue_key_tap(KEY_RIGHTARROW); break;
		case 'j': queue_key_tap(KEY_FIRE); break;
		case 'k': queue_key_tap(KEY_USE); break;
		case 'u': queue_key_tap(KEY_RSHIFT); break;
		case 'i': queue_key_tap(KEY_RALT); break;
		case 'q': queue_key_tap(KEY_ESCAPE); break;
		default:
			if (k >= 32 && k < 127)
				queue_key_tap(k);
			break;
		}
		break;
	}
	}
}

static void poll_input(void)
{
#if DG_RCORE_INTERACTIVE
	flush_pending_releases();
	for (int drained = 0; drained < KEYQUEUE_SIZE; ++drained) {
		long ch = syscall0(SYS_input_getchar);
		if (ch < 0)
			break;
		// Some QEMU stdio/monitor combinations may repeatedly surface NUL bytes
		// even when the user did not type anything. Treat them as "no useful input"
		// so the first frame is not blocked forever in input polling.
		if (ch == 0)
			break;
		handle_input_byte((unsigned char)ch);
	}
#endif
}

void DG_Init(void)
{
	uint32_t info[3] = {0, 0, 0};
	long ret = syscall2(SYS_fb_get_info, (long)info, 0);
	rcore_hart_snapshot snapshot;
	long snapshot_ret = syscall2(
		SYS_hart_snapshot, (long)&snapshot, (long)sizeof(snapshot));
	if (ret < 0) {
		printf("[doomgeneric] fb_get_info unavailable, game video may stay black\n");
		return;
	}
	printf("[doomgeneric] framebuffer: %ux%u stride=%u\n",
	       info[0], info[1], info[2]);
	if (snapshot_ret >= 0) {
		printf("[doomgeneric] online_harts=%lu current_hart=%lu online_mask=0x%lx\n",
		       (unsigned long)snapshot.online_harts,
		       (unsigned long)snapshot.current_hart,
		       (unsigned long)snapshot.online_mask);
	}
#if DG_RCORE_INTERACTIVE
	printf("[doomgeneric] mode=interactive (keyboard from cargo run terminal)\n");
	printf("[doomgeneric] note: single-threaded app; it may migrate across harts, but does not render in parallel on multiple harts\n");
	printf("[doomgeneric] controls: focus terminal, Q=menu, WASD/arrow=move, J=fire, K=use, U=run\n");
#else
	printf("[doomgeneric] mode=demo (-playdemo demo1)\n");
#endif
}

void DG_DrawFrame(void)
{
	if (!DG_ScreenBuffer)
		return;
	long ret = syscall2(SYS_fb_present, (long)DG_ScreenBuffer,
			    (long)(DOOMGENERIC_RESX * DOOMGENERIC_RESY * sizeof(pixel_t)));
	if (ret < 0 && s_fb_present_error_logs < 8) {
		printf("[doomgeneric] fb_present failed ret=%ld\n", ret);
		s_fb_present_error_logs++;
	}
}

void DG_SleepMs(uint32_t ms)
{
	rcore_timespec start, now;
	syscall2(SYS_clock_gettime, CLOCK_MONOTONIC, (long)&start);
	for (;;) {
		syscall2(SYS_clock_gettime, CLOCK_MONOTONIC, (long)&now);
		uint64_t t0 = (uint64_t)start.tv_sec * 1000000000ULL
			      + (uint64_t)start.tv_nsec;
		uint64_t t1 = (uint64_t)now.tv_sec * 1000000000ULL
			      + (uint64_t)now.tv_nsec;
		if (t1 - t0 >= (uint64_t)ms * 1000000ULL)
			break;
		syscall0(SYS_sched_yield);
	}
}

uint32_t DG_GetTicksMs(void)
{
	rcore_timespec tp;
	syscall2(SYS_clock_gettime, CLOCK_MONOTONIC, (long)&tp);
	return (uint32_t)(tp.tv_sec * 1000ULL + tp.tv_nsec / 1000000ULL);
}

int DG_GetKey(int *pressed, unsigned char *key)
{
#if !DG_RCORE_INTERACTIVE
	(void)pressed;
	(void)key;
	return 0;
#else
	if (s_key_r == s_key_w) {
		if (!s_poll_armed) {
			s_poll_armed = 1;
			return 0;
		}
		poll_input();
		s_poll_armed = 0;
	}
	if (s_key_r == s_key_w) {
		s_poll_armed = 1;
		return 0;
	}
	{
		unsigned short key_data = s_key_queue[s_key_r];
		s_key_r = (s_key_r + 1) % KEYQUEUE_SIZE;
		*pressed = key_data >> 8;
		*key = key_data & 0xff;
		return 1;
	}
#endif
}

void DG_SetWindowTitle(const char *title)
{
	(void)title;
}

/* Provide main - pass -iwad doom1.wad so Doom finds the WAD directly. */
#if DG_RCORE_INTERACTIVE
static char *fake_argv[] = {
	"doomgeneric",
	"-iwad", "doom1.wad",
	"-nomusic",
	"-nosound",
	NULL
};
#define FAKE_ARGC 5
#else
static char *fake_argv[] = {
	"doomgeneric", "-iwad", "doom1.wad", "-playdemo", "demo1", NULL
};
#define FAKE_ARGC 5
#endif

int main(int argc, char **argv)
{
	(void)argc;
	(void)argv;
	doomgeneric_Create(FAKE_ARGC, fake_argv);
	for (;;) {
		doomgeneric_Tick();
	}
}
