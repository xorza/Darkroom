cmake_minimum_required(VERSION 3.26)
project(nodeshop_editor)

set(CMAKE_CXX_STANDARD 23)


# Qt6 setup
set(CMAKE_AUTOMOC ON) # For meta-object compilation (moc)
set(CMAKE_AUTOUIC ON) # For user interface files compilation (uic)
set(CMAKE_AUTORCC ON) # For resource files compilation (rcc)
set(CMAKE_PREFIX_PATH "~/Qt/6.5.1/macos/lib/cmake/")
find_package(Qt6 COMPONENTS Core Quick Gui Widgets REQUIRED)


file(GLOB_RECURSE PROJECT_SOURCES ${CMAKE_SOURCE_DIR}/src/*.cpp)
add_executable(nodeshop_editor ${PROJECT_SOURCES} qml.qrc)
target_link_libraries(nodeshop_editor Qt6::Core Qt6::Quick Qt6::Gui Qt6::Widgets)
