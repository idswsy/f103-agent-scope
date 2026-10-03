/* tests/test_util.h —— 极简测试框架（与 proto/tests/test_vectors.c 同一套风格）
 *
 * 不引第三方框架：这个项目在 Windows + Git Bash 上开发，装什么都不方便，
 * 而这套宏已经够用 —— 一个计数、一个分组名、一条带颜色的失败信息。
 */

#ifndef TEST_UTIL_H
#define TEST_UTIL_H

#include <stdio.h>

static int g_pass = 0;
static int g_fail = 0;
static const char *g_group = "";

#define OK() do { g_pass++; } while (0)

#define FAIL(fmt, ...)                                                       \
    do {                                                                     \
        g_fail++;                                                            \
        fprintf(stderr, "  \033[31mFAIL\033[0m [%s] " fmt "\n", g_group,     \
                ##__VA_ARGS__);                                              \
    } while (0)

#define CHECK(cond, fmt, ...)                                                \
    do {                                                                     \
        if (cond) { OK(); } else { FAIL(fmt, ##__VA_ARGS__); }                \
    } while (0)

#define GROUP(name) do { g_group = (name); printf("\n[%s]\n", g_group); } while (0)

/* 在 main 末尾调用：打印汇总并返回退出码。 */
static int test_summary(void)
{
    printf("\n─────────────────────────────────────────────────\n");
    if (g_fail == 0) {
        printf("\033[32m全部通过\033[0m: %d 项\n", g_pass);
        return 0;
    }
    printf("\033[31m失败 %d 项\033[0m / 通过 %d 项\n", g_fail, g_pass);
    return 1;
}

#endif /* TEST_UTIL_H */
