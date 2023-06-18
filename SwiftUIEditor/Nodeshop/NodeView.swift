import SwiftUI

enum Direction {
    case input, output
}

struct PinButton: View {
    let name: String
    let direction: Direction
    
    @State private var isHovered = false
    
    var body: some View {
        Button(action: {
            print("\(name) button tapped")
        }) {
            HStack {
                if direction == .input {
                    Circle()
                        .fill(isHovered ? Color.blue : Color.orange)
                        .frame(width: 10, height: 10)
                        .overlay(
                            Circle()
                                .stroke(Color.black, lineWidth: 2)
                        )
                }
                
                Text(name)
                
                if direction == .output {
                    Circle()
                        .fill(isHovered ? Color.blue : Color.orange)
                        .frame(width: 10, height: 10)
                        .overlay(
                            Circle()
                                .stroke(Color.black, lineWidth: 2)
                        )
                }
            }
        }
        .buttonStyle(PlainButtonStyle())
        .onHover { hover in
            isHovered = hover
        }
    }
}

struct NodeView: View {
    let name:String
    let inputs:[String]
    let outputs:[String]
    
    var body: some View {
        VStack(alignment: .center){
            
            Text(name)
                .font(.headline)
                .padding(.vertical, 5.0)
            
            
            
            
            //                .overlay(
            //                    Rectangle()
            //                        .fill(Color.blue)
            //                )
            
            HStack(alignment: .top, spacing: 10.0){
                VStack{
                    ForEach(inputs, id: \.self) { name in
                        PinButton(name: name, direction: .input)
                    }
                }
                
                VStack(alignment: .trailing) {
                    ForEach(outputs, id: \.self) { name in
                        PinButton(name: name, direction: .output)
                    }
                }
            }
            .padding([.leading, .bottom, .trailing], 10.0)
        }
        .overlay(
            RoundedRectangle(cornerRadius: 5)
                .stroke(Color.blue, lineWidth: 1)
        )
    }
}

struct NodeView_Previews: PreviewProvider {
    static var previews: some View {
        NodeView(
            name:"Add",
            inputs: ["input a", "input b"],
            outputs: ["output 1", "output 2"]
        )
    }
}
