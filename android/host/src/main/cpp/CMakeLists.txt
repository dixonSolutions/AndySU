cmake_minimum_required(VERSION 3.22)
project(andysu_host C)
add_library(andysu_host SHARED andysu_host.c)
target_include_directories(andysu_host PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/../../../../../desktop/include)
target_link_libraries(andysu_host log dl)
# 16 KB pages (Android 15+): align LOAD segments to 16 KB on every ABI.
target_link_options(andysu_host PRIVATE "-Wl,-z,max-page-size=16384")
