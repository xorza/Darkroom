include(FetchContent)
FetchContent_Declare(
        googletest
        GIT_REPOSITORY https://github.com/google/googletest.git
        GIT_TAG        v1.13.0
)
FetchContent_MakeAvailable(googletest)

enable_testing()

file(GLOB_RECURSE PROJECT_SOURCES ./*.cpp)
add_executable(tests ${PROJECT_SOURCES})
target_link_libraries(tests gtest_main)
add_test(NAME tests COMMAND tests)
