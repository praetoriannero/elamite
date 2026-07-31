#ifndef ELAMITE_SDL_DEMO_H
#define ELAMITE_SDL_DEMO_H

#include <stdint.h>

typedef struct elamite_sdl_app elamite_sdl_app;

elamite_sdl_app *elamite_sdl_open(int32_t width, int32_t height);
int32_t elamite_sdl_poll(elamite_sdl_app *app);
void elamite_sdl_draw(elamite_sdl_app *app);
void elamite_sdl_close(elamite_sdl_app *app);

#endif
