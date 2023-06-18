cmake_minimum_required(VERSION 3.26)
project(editor)


option(ENABLE_TESTS "Enable tests" ON)
option(ENABLE_MODULES "Enable modules" OFF)


set(CMAKE_CXX_STANDARD 23)

if (ENABLE_MODULES)
    set(CMAKE_CXX_STANDARD_REQUIRED ON)
    set(CMAKE_CXX_EXTENSIONS OFF)
    set(CMAKE_EXPERIMENTAL_CXX_MODULE_CMAKE_API 2182bf5c-ef0d-489a-91da-49dbc3090d2a)
    set(CMAKE_EXPERIMENTAL_CXX_MODULE_DYNDEP 1)
endif ()


# Qt6 setup
set(CMAKE_AUTOMOC ON) # For meta-object compilation (moc)
set(CMAKE_AUTOUIC ON) # For user interface files compilation (uic)
set(CMAKE_AUTORCC ON) # For resource files compilation (rcc)
set(CMAKE_PREFIX_PATH "~/Qt/6.5.1/macos/lib/cmake/")
find_package(Qt6 COMPONENTS Core Quick Qml REQUIRED)


add_library(editor_lib)
add_executable(editor_exe)
target_link_libraries(editor_exe editor_lib)


target_link_libraries(editor_lib Qt6::Core Qt6::Quick Qt6::Qml)


if (ENABLE_MODULES)
    file(GLOB_RECURSE PROJECT_MODULES ${CMAKE_SOURCE_DIR}/src/*.cxx)
    target_sources(
            editor_lib PRIVATE
            FILE_SET cxx_modules TYPE CXX_MODULES FILES
            ${PROJECT_MODULES}
    )
endif ()


file(GLOB_RECURSE PROJECT_SOURCES ${CMAKE_SOURCE_DIR}/src/*.cpp)
target_sources(editor_lib PRIVATE ${PROJECT_SOURCES})
target_include_directories(editor_lib PRIVATE ${CMAKE_SOURCE_DIR}/src/)
target_sources(editor_exe PRIVATE qml.qrc)


# Graph library dependency
set(GRAPH_LIB_NAME "libgraph${CMAKE_SHARED_LIBRARY_SUFFIX}")
set(GRAPH_LIB_PATH_DEBUG ${CMAKE_SOURCE_DIR}/../Graph/target/debug)
set(GRAPH_LIB_PATH_RELEASE ${CMAKE_SOURCE_DIR}/../Graph/target/release)
if (CMAKE_BUILD_TYPE STREQUAL "Debug")
    set(CARGO_BUILD_TYPE "")
elseif (CMAKE_BUILD_TYPE STREQUAL "Release" OR CMAKE_BUILD_TYPE STREQUAL "RelWithDebInfo")
    set(CARGO_BUILD_TYPE "release")
endif ()
add_custom_target(
        graph_lib_target
        COMMAND cargo build --${CARGO_BUILD_TYPE}
        WORKING_DIRECTORY ${CMAKE_SOURCE_DIR}/../Graph
)
add_dependencies(editor_lib graph_lib_target)
target_link_libraries(
        editor_lib
        debug ${GRAPH_LIB_PATH_DEBUG}/${GRAPH_LIB_NAME}
        optimized ${GRAPH_LIB_PATH_RELEASE}/${GRAPH_LIB_NAME}
)


if (ENABLE_TESTS)
    add_subdirectory(tests)
    target_link_libraries(tests editor_lib)
endif ()
