#include "elamite_sdl_demo.h"

#include <SDL2/SDL.h>
#include <stddef.h>
#include <stdint.h>

struct elamite_sdl_app {
    SDL_Window *window;
    SDL_Renderer *renderer;
    int32_t width;
    int32_t height;
};

static elamite_sdl_app demo_app;
static int32_t sdl_is_open;

static void destroy_app(void)
{
    if (demo_app.renderer != NULL) {
        SDL_DestroyRenderer(demo_app.renderer);
        demo_app.renderer = NULL;
    }
    if (demo_app.window != NULL) {
        SDL_DestroyWindow(demo_app.window);
        demo_app.window = NULL;
    }
}

elamite_sdl_app *elamite_sdl_open(int32_t width, int32_t height)
{
    if (sdl_is_open != 0) {
        return &demo_app;
    }
    if (width <= 0 || height <= 0 || SDL_Init(SDL_INIT_VIDEO) != 0) {
        return NULL;
    }

    demo_app.width = width;
    demo_app.height = height;
    demo_app.window = SDL_CreateWindow(
        "Elamite SDL demo",
        SDL_WINDOWPOS_CENTERED,
        SDL_WINDOWPOS_CENTERED,
        width,
        height,
        SDL_WINDOW_SHOWN
    );
    if (demo_app.window == NULL) {
        destroy_app();
        SDL_Quit();
        return NULL;
    }

    demo_app.renderer = SDL_CreateRenderer(
        demo_app.window,
        -1,
        SDL_RENDERER_ACCELERATED | SDL_RENDERER_PRESENTVSYNC
    );
    if (demo_app.renderer == NULL) {
        demo_app.renderer = SDL_CreateRenderer(demo_app.window, -1, SDL_RENDERER_SOFTWARE);
    }
    if (demo_app.renderer == NULL) {
        destroy_app();
        SDL_Quit();
        return NULL;
    }

    sdl_is_open = 1;
    return &demo_app;
}

int32_t elamite_sdl_poll(elamite_sdl_app *app)
{
    SDL_Event event;

    if (app != &demo_app || sdl_is_open == 0) {
        return 0;
    }

    while (SDL_PollEvent(&event) != 0) {
        if (event.type == SDL_QUIT) {
            return 0;
        }
        if (event.type == SDL_KEYDOWN && event.key.keysym.sym == SDLK_ESCAPE) {
            return 0;
        }
    }

    return 1;
}

void elamite_sdl_draw(elamite_sdl_app *app)
{
    uint32_t ticks;
    int32_t box_size;
    int32_t travel;
    int32_t box_x;
    int32_t wave;
    int32_t box_y;
    uint8_t pulse;
    SDL_Rect box;
    SDL_Rect horizon;

    if (app != &demo_app || sdl_is_open == 0) {
        return;
    }

    ticks = SDL_GetTicks();
    box_size = 88;
    travel = app->width + box_size;
    box_x = (int32_t)((ticks / 4U) % (uint32_t)travel) - box_size;
    wave = (int32_t)((ticks / 7U) % 160U);
    box_y = app->height / 2 - box_size / 2 + wave - 80;
    pulse = (uint8_t)((ticks / 8U) % 128U);
    box = (SDL_Rect){box_x, box_y, box_size, box_size};
    horizon = (SDL_Rect){0, app->height * 3 / 4, app->width, app->height / 4};

    (void)SDL_SetRenderDrawColor(app->renderer, 12U, 18U, 38U, 255U);
    (void)SDL_RenderClear(app->renderer);

    (void)SDL_SetRenderDrawColor(app->renderer, 28U, 48U, 78U, 255U);
    (void)SDL_RenderFillRect(app->renderer, &horizon);

    (void)SDL_SetRenderDrawColor(app->renderer, (uint8_t)(127U + pulse), 92U, 220U, 255U);
    (void)SDL_RenderFillRect(app->renderer, &box);

    (void)SDL_SetRenderDrawColor(app->renderer, 242U, 210U, 96U, 255U);
    (void)SDL_RenderDrawRect(app->renderer, &box);

    SDL_RenderPresent(app->renderer);
}

void elamite_sdl_close(elamite_sdl_app *app)
{
    if (app != &demo_app || sdl_is_open == 0) {
        return;
    }

    destroy_app();
    SDL_Quit();
    sdl_is_open = 0;
}
