# A consumer of the installed package: one native payload plus a tiny program that calls the core.
cmake_minimum_required(VERSION 3.16)
project(andysu_c_example C)
find_package(AndySU CONFIG REQUIRED)
add_library(hello SHARED hello.c)
target_link_libraries(hello PRIVATE AndySU::payload)
target_link_options(hello PRIVATE "-Wl,-z,max-page-size=16384")
add_executable(core_version core_version.c)
target_link_libraries(core_version PRIVATE AndySU::core)
