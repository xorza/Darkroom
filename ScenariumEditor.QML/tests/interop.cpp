#include "../src/utils/uuid.hpp"
#include "../src/CoreContext.hpp"


#include <iostream>
#include <string>

#include <catch2/catch_test_macros.hpp>


TEST_CASE("Uuid tests", "[uuid]") {
    auto uuid = uuid::new_v4();
    auto uuid_str = uuid.to_string();
    auto uuid2 = uuid::from_string(uuid_str);
    REQUIRE(uuid == uuid2);

    auto uuid_str2 = "b5bd73a8-ee5b-46b8-9464-ed0915dad6d7";
    auto uuid3 = uuid::from_string(uuid_str2);
    REQUIRE(uuid3.to_string() == uuid_str2);
}


TEST_CASE("New node 1", "[context]") {
    auto ctx = Ctx{};

    auto nodes1 = ctx.get_nodes();

    auto funcs = ctx.get_funcs();
    auto new_node = ctx.new_node(funcs[0].id);

    auto nodes2 = ctx.get_nodes();

    REQUIRE(nodes1.size() + 1 == nodes2.size());
}