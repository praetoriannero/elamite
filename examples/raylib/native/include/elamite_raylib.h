#ifndef ELAMITE_RAYLIB_H
#define ELAMITE_RAYLIB_H

#include <stdint.h>
#include <raylib.h>

/*
 * Elamite intentionally excludes C bool and text from its initial ABI-safe
 * types. Keep those conversions explicit and tiny while calling the remaining
 * raylib API directly from Elamite.
 */
static inline void elamite_raylib_init(int32_t width, int32_t height) {
    InitWindow((int)width, (int)height, "Elamite + raylib");
    SetTargetFPS(60);
}

static inline int32_t elamite_raylib_should_close(void) {
    return WindowShouldClose() ? 1 : 0;
}

static inline int32_t elamite_raylib_horizontal_input(void) {
    return (IsKeyDown(KEY_RIGHT) ? 1 : 0) - (IsKeyDown(KEY_LEFT) ? 1 : 0);
}

static inline int32_t elamite_raylib_vertical_input(void) {
    return (IsKeyDown(KEY_DOWN) ? 1 : 0) - (IsKeyDown(KEY_UP) ? 1 : 0);
}

static inline void elamite_raylib_draw_instructions(void) {
    DrawText("Move the circle with the arrow keys", 20, 20, 20, DARKGRAY);
}

static inline void elamite_raylib_clear_background(void) {
    ClearBackground((Color){245, 245, 240, 255});
}

static inline void elamite_raylib_draw_border(void) {
    DrawRectangleLines(10, 10, 780, 430, (Color){170, 175, 185, 255});
}

static inline void elamite_raylib_draw_circle(float x, float y, float radius) {
    DrawCircleV((Vector2){x, y}, radius, (Color){80, 130, 220, 255});
}

#endif
