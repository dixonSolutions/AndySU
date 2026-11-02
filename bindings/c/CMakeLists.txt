# AndySU C/CMake package.
#   AndySU::payload  header-only payload contract (andysu_payload.h, ANDYSU_PAYLOAD)
#   AndySU::core     the core's C ABI (andysu.h + libandysu_core.a) for hosts written in C
# Build and install:  cmake -S bindings/c -B build -DCMAKE_INSTALL_PREFIX=dist && cmake --build build --target install
# Consume:            find_package(AndySU CONFIG REQUIRED) / FetchContent (see README.md)
cmake_minimum_required(VERSION 3.16)
file(READ "${CMAKE_CURRENT_SOURCE_DIR}/../../VERSION" ANDYSU_VERSION_MM)
string(STRIP "${ANDYSU_VERSION_MM}" ANDYSU_VERSION_MM)
set(ANDYSU_VERSION "${ANDYSU_VERSION_MM}.0" CACHE STRING "Package version")
project(AndySU VERSION ${ANDYSU_VERSION} LANGUAGES C)

set(REPO "${CMAKE_CURRENT_SOURCE_DIR}/../..")
option(ANDYSU_WITH_CORE "Build and install the core static library (needs cargo)" ON)

include(GNUInstallDirs)
add_library(andysu_payload INTERFACE)
add_library(AndySU::payload ALIAS andysu_payload)
set_target_properties(andysu_payload PROPERTIES EXPORT_NAME payload)
target_include_directories(andysu_payload INTERFACE
  $<BUILD_INTERFACE:${REPO}/desktop/include> $<INSTALL_INTERFACE:${CMAKE_INSTALL_INCLUDEDIR}>)
install(FILES ${REPO}/desktop/include/andysu_payload.h DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
install(TARGETS andysu_payload EXPORT AndySUTargets)

if(ANDYSU_WITH_CORE)
  set(CORE_A ${REPO}/target/release/libandysu_core.a)
  add_custom_command(OUTPUT ${CORE_A}
    COMMAND cargo build --release -p andysu-core --features ffi,transport
    WORKING_DIRECTORY ${REPO} COMMENT "Building libandysu_core.a")
  add_custom_target(andysu_core_build ALL DEPENDS ${CORE_A})
  add_library(andysu_core STATIC IMPORTED GLOBAL)
  set_target_properties(andysu_core PROPERTIES IMPORTED_LOCATION ${CORE_A})
  install(FILES ${CORE_A} DESTINATION ${CMAKE_INSTALL_LIBDIR})
  install(FILES ${REPO}/core/include/andysu.h DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})
endif()

install(EXPORT AndySUTargets NAMESPACE AndySU:: DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/AndySU)
include(CMakePackageConfigHelpers)
configure_package_config_file(cmake/AndySUConfig.cmake.in ${CMAKE_CURRENT_BINARY_DIR}/AndySUConfig.cmake
  INSTALL_DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/AndySU)
write_basic_package_version_file(${CMAKE_CURRENT_BINARY_DIR}/AndySUConfigVersion.cmake COMPATIBILITY SameMinorVersion)
install(FILES ${CMAKE_CURRENT_BINARY_DIR}/AndySUConfig.cmake ${CMAKE_CURRENT_BINARY_DIR}/AndySUConfigVersion.cmake
  DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/AndySU)
