cmake_minimum_required(VERSION 3.28)

project(scenarium_editor VERSION 0.1 LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
include(FetchContent)
#set(CMAKE_CXX_SCAN_FOR_MODULES ON)  # enable this for cpp20 modules


if (DEFINED ENV{Qt6})
    set(CMAKE_PREFIX_PATH $ENV{Qt6})
    message(STATUS "Qt6 is set to ${CMAKE_PREFIX_PATH}")
else ()
    message(FATAL_ERROR "Qt6 environment variable is not set")
endif ()


# find qt
find_package(Qt6 6.7 REQUIRED COMPONENTS Quick)
qt_standard_project_setup()
qt_policy(SET QTP0001 NEW)

# Fetch protobuf
set(protobuf_BUILD_TESTS OFF)
set(protobuf_INSTALL ON)
set(protobuf_BUILD_EXAMPLES OFF)
set(protobuf_BUILD_PROTOBUF_BINARIES ON)
set(protobuf_BUILD_PROTOC_BINARIES OFF)
set(protobuf_BUILD_LIBUPB OFF)
set(protobuf_BUILD_LIBUPB OFF)
set(protobuf_BUILD_SHARED_LIBS_DEFAULT OFF)
set(ABSL_PROPAGATE_CXX_STD ON)

FetchContent_Declare(
        protobuf
        GIT_REPOSITORY https://github.com/protocolbuffers/protobuf.git
        GIT_TAG v27.1
)
FetchContent_MakeAvailable(protobuf)

# Fetch Catch2
FetchContent_Declare(
        catch2
        GIT_REPOSITORY https://github.com/catchorg/Catch2.git
        GIT_TAG v3.6.0
)
FetchContent_MakeAvailable(catch2)


if (${CMAKE_SYSTEM_NAME} STREQUAL "Windows")
    set(LIBB_CS_INTEROP_FILE "${CMAKE_SOURCE_DIR}/../target/x86_64-pc-windows-gnu/release/libcs_interop.dll.a")
    set(LIBB_CS_INTEROP_DYLIB_FILE "${CMAKE_SOURCE_DIR}/../target/x86_64-pc-windows-gnu/release/cs_interop.dll")
elseif (${CMAKE_SYSTEM_NAME} STREQUAL "Darwin")
    set(LIBB_CS_INTEROP_FILE "${CMAKE_SOURCE_DIR}/../target/release/libcs_interop.dylib")
    set(LIBB_CS_INTEROP_DYLIB_FILE "${CMAKE_SOURCE_DIR}/../target/release/libcs_interop.dylib")
endif ()


# build Rust project
add_custom_command(
        OUTPUT ${LIBB_CS_INTEROP_FILE}
        COMMAND cargo build --release --target x86_64-pc-windows-gnu
        WORKING_DIRECTORY ${CMAKE_SOURCE_DIR}/../cs_interop
        COMMENT "Building cs_interop Rust project"
        VERBATIM
)
add_custom_target(build_rust ALL DEPENDS ${LIBB_CS_INTEROP_FILE})


# generate the protobuf code
add_custom_command(
        OUTPUT ${CMAKE_SOURCE_DIR}/../cs_interop/proto/graph.pb.h ${CMAKE_SOURCE_DIR}/../cs_interop/proto/graph.pb.cc
        COMMAND protoc.exe --cpp_out . --proto_path . graph.proto
        WORKING_DIRECTORY ${CMAKE_SOURCE_DIR}/../cs_interop/proto
        DEPENDS ${CMAKE_SOURCE_DIR}/../cs_interop/proto/graph.proto
        COMMENT "Generating protobuf code"
)
add_custom_target(generate_protobuf ALL DEPENDS
        ${CMAKE_SOURCE_DIR}/../cs_interop/proto/graph.pb.h
        ${CMAKE_SOURCE_DIR}/../cs_interop/proto/graph.pb.cc
)



file(GLOB_RECURSE SRC_FILES ${CMAKE_CURRENT_SOURCE_DIR} src/*.hpp src/*.cpp)
file(GLOB SRC_PB_FILES ${CMAKE_SOURCE_DIR} ../cs_interop/proto/*.cc)
qt_add_executable(scenarium_editor ${SRC_FILES} ${SRC_PB_FILES} main.cpp)
set_target_properties(scenarium_editor PROPERTIES ENABLE_EXPORTS ON) # for tests
add_dependencies(scenarium_editor generate_protobuf)
add_dependencies(scenarium_editor build_rust)


if (${CMAKE_SYSTEM_NAME} STREQUAL "Windows")
    target_link_options(scenarium_editor
            PUBLIC
            -static -static-libgcc -static-libstdc++
    )
endif ()


file(GLOB_RECURSE QML_FILES RELATIVE ${CMAKE_CURRENT_SOURCE_DIR} qml/*.qml qml/*.js)
file(GLOB_RECURSE SRC_QML_FILES RELATIVE ${CMAKE_CURRENT_SOURCE_DIR} qml/*.hpp qml/*.cpp)
file(GLOB_RECURSE RESOURCES RELATIVE ${CMAKE_CURRENT_SOURCE_DIR}
        assets/*.png assets/*.jpg assets/*.svg
)
qt_add_qml_module(scenarium_editor
        URI scenarium_editor
        VERSION 1.0

        QML_FILES
        ${QML_FILES}

        RESOURCES

        SOURCES
        ${SRC_QML_FILES}
)

target_compile_options(scenarium_editor PRIVATE
        #-fno-exceptions
        -fno-rtti
        -Wall
        -Wextra
        -Wpedantic
        -Werror
        -Wno-unused-parameter
        -Wno-unused-variable
        -Wno-unused-function
        -Wno-unused-but-set-variable
        -Wno-unused-value
        -Wno-unused-local-typedefs
        #-Wno-unused-private-field
        -Wno-unused-const-variable
)


target_include_directories(scenarium_editor PUBLIC
        lib
        fbs
        ${CMAKE_SOURCE_DIR}/../cs_interop/proto
)

target_link_libraries(scenarium_editor
        PUBLIC
        Qt6::Quick
        protobuf::libprotobuf
        ${LIBB_CS_INTEROP_FILE}
)

# Copy the assets to the build directory
set_target_properties(scenarium_editor PROPERTIES
        MACOSX_BUNDLE_GUI_IDENTIFIER com.cssodessa.scenarium_editor
        MACOSX_BUNDLE_BUNDLE_VERSION ${PROJECT_VERSION}
        MACOSX_BUNDLE_SHORT_VERSION_STRING ${PROJECT_VERSION_MAJOR}.${PROJECT_VERSION_MINOR}
        MACOSX_BUNDLE TRUE
        WIN32_EXECUTABLE TRUE
)
include(GNUInstallDirs)
install(TARGETS scenarium_editor
        BUNDLE DESTINATION .
        LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
        RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
)


# Copy the DLL file to the build directory
add_custom_command(
        TARGET scenarium_editor POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
        ${LIBB_CS_INTEROP_DYLIB_FILE}
        $<TARGET_FILE_DIR:scenarium_editor>
        COMMENT "Copying DLL file to build directory"
)


# Tests
file(GLOB_RECURSE TEST_SRC_FILES "tests/*.cpp")
add_executable(scenarium-tests ${SRC_FILES} ${SRC_PB_FILES} ${TEST_SRC_FILES})
target_link_libraries(scenarium-tests PRIVATE
        Catch2::Catch2WithMain
        ${LIBB_CS_INTEROP_FILE}
        protobuf::libprotobuf
)
target_include_directories(scenarium-tests PRIVATE
        lib
        fbs
        ${CMAKE_SOURCE_DIR}/../cs_interop/proto
)
add_dependencies(scenarium-tests generate_protobuf)
add_dependencies(scenarium-tests build_rust)
#list(APPEND CMAKE_MODULE_PATH ${catch2_SOURCE_DIR}/extras)
include(CTest)
include(Catch)
catch_discover_tests(scenarium-tests)

