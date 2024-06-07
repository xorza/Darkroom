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
if (${CMAKE_SYSTEM_NAME} STREQUAL "Windows")
    target_link_options(scenarium_editor
            PUBLIC
            -static -static-libgcc -static-libstdc++
    )
endif ()

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
