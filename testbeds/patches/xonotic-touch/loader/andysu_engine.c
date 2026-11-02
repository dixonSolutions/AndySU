/*
 * Engine side of the loader ABI (andysu_swap.h). Stage 1: run the engine once.
 * Compiled into libmain.so with the rest of darkplaces; libmain exports only andysu_engine_v1
 * (version script andysu_engine.map).
 */
#include "darkplaces.h"
#include "andysu_swap.h"

extern void Andysu_InputDevices(int touch, int keyboard, int mouse); /* vid_touchdetect.c */

static const andysu_loader_api *andysu_loader;

static int andysu_run(const andysu_loader_api *loader, int argc, char **argv, int swapped_in)
{
	andysu_loader = loader;
	if (loader && loader->emit)
		loader->emit(swapped_in ? "{\"event\":\"engine_entry\",\"build_id\":\"" ANDYSU_BUILD_ID "\",\"swapped\":1}"
		                        : "{\"event\":\"engine_entry\",\"build_id\":\"" ANDYSU_BUILD_ID "\",\"swapped\":0}");
	return Sys_Main(argc, argv);
}

static const andysu_engine andysu_engine_desc = {
	ANDYSU_ENGINE_ABI, sizeof(andysu_engine), 1, ANDYSU_BUILD_ID, andysu_run, Andysu_InputDevices,
};

__attribute__((visibility("default"))) const andysu_engine *andysu_engine_v1(void)
{
	return &andysu_engine_desc;
}
