cmake_minimum_required(VERSION 3.28)

project(scenarium_editor VERSION 0.1 LANGUAGES CXX)
set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
#set(CMAKE_CXX_SCAN_FOR_MODULES ON)  # enable this for cpp20 modules


if (DEFINED ENV{Qt6})
    set(CMAKE_PREFIX_PATH $ENV{Qt6})
    message("-- Qt6 is set to ${CMAKE_PREFIX_PATH}")
else ()
    message(FATAL_ERROR "Qt6 environment variable is not set")
endif ()


find_package(Qt6 6.7 REQUIRED COMPONENTS Quick)
qt_standard_project_setup()
qt_policy(SET QTP0001 NEW)


file(GLOB_RECURSE SRC_FILES RELATIVE ${CMAKE_CURRENT_SOURCE_DIR} src/*.hpp src/*.cpp)
qt_add_executable(scenarium_editor ${SRC_FILES} main.cpp)
set_target_properties(scenarium_editor PROPERTIES ENABLE_EXPORTS ON) # for tests

file(GLOB_RECURSE QML_FILES RELATIVE ${CMAKE_CURRENT_SOURCE_DIR} qml/*.qml)
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
target_link_libraries(scenarium_editor
        PUBLIC
        Qt6::Quick
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

if (CMAKE_BUILD_TYPE STREQUAL "Debug")
    target_link_libraries(scenarium_editor
            PUBLIC
            ${CMAKE_SOURCE_DIR}/../target/x86_64-pc-windows-gnu/debug/libcs_interop.dll.a
    )
else ()
    target_link_libraries(scenarium_editor
            PUBLIC
            ${CMAKE_SOURCE_DIR}/../target/x86_64-pc-windows-gnu/release/libcs_interop.dll.a
    )
endif ()

target_include_directories(scenarium_editor PUBLIC lib)

if (${CMAKE_SYSTEM_NAME} STREQUAL "Windows")
    target_link_options(scenarium_editor
            PUBLIC
            -static -static-libgcc -static-libstdc++
    )
endif ()


add_custom_command(
        TARGET scenarium_editor POST_BUILD
        COMMAND ${CMAKE_COMMAND} -E copy_if_different
        ${CMAKE_SOURCE_DIR}/../target/x86_64-pc-windows-gnu/debug/cs_interop.dll
        $<TARGET_FILE_DIR:scenarium_editor>
        COMMENT "Copying DLL file to build directory"
)

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



# Tests
include(FetchContent)
FetchContent_Declare(
        catch2
        GIT_REPOSITORY https://github.com/catchorg/Catch2.git
        GIT_TAG v3.6.0
)
FetchContent_MakeAvailable(catch2)

file(GLOB_RECURSE TEST_SRC_FILES "tests/*.cpp")
add_executable(tests ${SRC_FILES} ${TEST_SRC_FILES})
target_link_libraries(tests PRIVATE
        Catch2::Catch2WithMain
        ${CMAKE_SOURCE_DIR}/../target/x86_64-pc-windows-gnu/debug/libcs_interop.dll.a
)
#list(APPEND CMAKE_MODULE_PATH ${catch2_SOURCE_DIR}/extras)
include(CTest)
include(Catch)
catch_discover_tests(tests)
