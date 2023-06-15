cmake_minimum_required(VERSION 3.26)
project(editor)

set(CMAKE_CXX_STANDARD 23)


# Qt6 setup
set(CMAKE_AUTOMOC ON) # For meta-object compilation (moc)
set(CMAKE_AUTOUIC ON) # For user interface files compilation (uic)
set(CMAKE_AUTORCC ON) # For resource files compilation (rcc)
set(CMAKE_PREFIX_PATH "~/Qt/6.5.1/macos/lib/cmake/")
find_package(Qt6 COMPONENTS Core Quick Gui Qml REQUIRED)


file(GLOB_RECURSE PROJECT_SOURCES ${CMAKE_SOURCE_DIR}/src/*.cpp)
add_executable(editor ${PROJECT_SOURCES} qml.qrc)
target_link_libraries(editor Qt6::Core Qt6::Quick Qt6::Gui Qt6::Qml)


set(GRAPH_LIB_NAME graph)
set(GRAPH_LIB_PATH ${CMAKE_SOURCE_DIR}/../Graph/target/debug)
add_custom_target(
        graph_lib_target
        COMMAND cargo build
        WORKING_DIRECTORY ${CMAKE_SOURCE_DIR}/../Graph
)
add_dependencies(editor graph_lib_target)
target_link_libraries(editor ${GRAPH_LIB_PATH}/lib${GRAPH_LIB_NAME} ${CMAKE_SHARED_LIBRARY_SUFFIX})
